//! The keys that change the list's shape: `f` sections, `o`/`O` sort, `v`
//! group-by. Every change is written back to the server settings the web UI
//! reads, so the two agree on the next reload.

use super::action::Effect;
use super::list_view::{SECTIONS, next_sort};
use super::state::{App, View};

#[derive(Debug, Clone, Copy)]
pub enum ListShapeAction {
    ToggleSectionsMenu,
    SectionsNext,
    SectionsPrev,
    SectionsToggle,
    CycleSort,
    FlipSortDir,
    CycleGroupBy,
    CycleColorBy,
}

pub fn reduce(app: &mut App, action: ListShapeAction) -> Vec<Effect> {
    match action {
        ListShapeAction::ToggleSectionsMenu => {
            if app.sections_menu.take().is_some() || app.view() != View::SessionList {
                return Vec::new();
            }
            app.sections_menu = Some(0);
            Vec::new()
        }
        ListShapeAction::SectionsNext => {
            move_menu(app, 1);
            Vec::new()
        }
        ListShapeAction::SectionsPrev => {
            move_menu(app, -1);
            Vec::new()
        }
        ListShapeAction::SectionsToggle => {
            let Some(at) = app.sections_menu else { return Vec::new() };
            let Some(section) = SECTIONS.get(at).copied() else { return Vec::new() };
            app.list_shape.sections.toggle(section);
            settle(app)
        }
        ListShapeAction::CycleSort => {
            let chosen = app.list_shape.sort.next();
            let (sort, dir) = next_sort(app.list_shape.sort, app.list_shape.sort_dir, chosen);
            app.list_shape.sort = sort;
            app.list_shape.sort_dir = dir;
            settle(app)
        }
        ListShapeAction::FlipSortDir => {
            app.list_shape.sort_dir = app.list_shape.sort_dir.flipped();
            settle(app)
        }
        ListShapeAction::CycleGroupBy => {
            app.list_shape.group_by = app.list_shape.group_by.next();
            settle(app)
        }
        ListShapeAction::CycleColorBy => {
            app.list_shape.color_by = next_color_by(app.list_shape.color_by);
            settle(app)
        }
    }
}

/// The accent dimension steps through the same keys as the grouping, plus off.
fn next_color_by(current: super::list_view::ColorBy) -> super::list_view::ColorBy {
    use super::list_view::ColorBy;
    const CYCLE: &[ColorBy] =
        &[ColorBy::None, ColorBy::Machine, ColorBy::Label, ColorBy::WorkingDir, ColorBy::Room];
    let at = CYCLE.iter().position(|c| *c == current).unwrap_or(0);
    CYCLE[(at + 1) % CYCLE.len()]
}

fn move_menu(app: &mut App, delta: i32) {
    let len = SECTIONS.len();
    let at = app.sections_menu.unwrap_or(0);
    let next = if delta < 0 { at.checked_sub(1).unwrap_or(len - 1) } else { (at + 1) % len };
    app.sections_menu = Some(next);
}

/// Re-shapes the list and persists. The row set moved, so the selection and the
/// viewport have to be recomputed before anything reads them.
fn settle(app: &mut App) -> Vec<Effect> {
    app.reshape();
    vec![save(app)]
}

/// Sends only this view's own keys, under `sessionList`.
fn save(app: &mut App) -> Effect {
    let patch = serde_json::json!({"sessionList": app.list_shape.settings_patch()});
    if let Some(blob) = app.settings_blob.as_mut() {
        super::settings_write::deep_merge(blob, patch.clone());
    }
    super::settings_write::save(patch)
}

#[cfg(test)]
mod tests {
    use super::{ListShapeAction, reduce};
    use crate::app::action::Effect;
    use crate::app::list_view::{ColorBy, GroupBy, Section, Sort, SortDir};
    use crate::app::state::App;
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn saved_list(effects: &[Effect]) -> serde_json::Value {
        match effects {
            [Effect::SaveSettings { patch }] => patch["sessionList"].clone(),
            _ => panic!("expected one settings write, got {} effects", effects.len()),
        }
    }

    #[test]
    fn the_sort_key_cycles_and_persists_every_step() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::CycleSort);
        assert_eq!(app.list_shape.sort, Sort::Created);
        assert_eq!(saved_list(&effects)["sort"], "created");

        reduce(&mut app, ListShapeAction::CycleSort);
        assert_eq!(app.list_shape.sort, Sort::Name);
        assert_eq!(app.list_shape.sort_dir, SortDir::Asc, "a new field takes its natural order");
    }

    #[test]
    fn the_direction_key_flips_without_changing_the_field() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::FlipSortDir);
        assert_eq!(app.list_shape.sort, Sort::Activity);
        assert_eq!(app.list_shape.sort_dir, SortDir::Asc);
        assert_eq!(saved_list(&effects)["sortDir"], "asc");
    }

    #[test]
    fn the_group_and_colour_keys_cycle_and_persist() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::CycleGroupBy);
        assert_eq!(app.list_shape.group_by, GroupBy::Label);
        assert_eq!(saved_list(&effects)["groupBy"], "label");

        let effects = reduce(&mut app, ListShapeAction::CycleColorBy);
        assert_eq!(app.list_shape.color_by, ColorBy::Machine);
        assert_eq!(saved_list(&effects)["colorBy"], "machine");
    }

    #[test]
    fn the_sections_popup_opens_closes_and_wraps() {
        let mut app = app();
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        assert_eq!(app.sections_menu, Some(0));
        reduce(&mut app, ListShapeAction::SectionsPrev);
        assert_eq!(app.sections_menu, Some(crate::app::list_view::SECTIONS.len() - 1));
        reduce(&mut app, ListShapeAction::SectionsNext);
        assert_eq!(app.sections_menu, Some(0));
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        assert!(app.sections_menu.is_none(), "the same key closes it");
    }

    #[test]
    fn toggling_a_section_reshapes_the_list_and_persists() {
        let mut app = app();
        assert_eq!(app.flattened_sessions().len(), 2);
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        // Step to Live, the bucket both fixtures sit in.
        reduce(&mut app, ListShapeAction::SectionsNext);
        let effects = reduce(&mut app, ListShapeAction::SectionsToggle);
        assert!(!app.list_shape.sections.has(Section::Live));
        assert!(app.flattened_sessions().is_empty(), "the rows went with the section");
        assert_eq!(saved_list(&effects)["section"], "starred,dispatched");
    }

    /// The write names only `sessionList` keys. Keeping `display` and the
    /// web-UI-only siblings is the merge's job, against a read taken at write
    /// time — see `settings_write`.
    #[test]
    fn a_write_carries_only_this_views_own_keys() {
        let mut app = app();
        app.settings_blob = Some(serde_json::json!({
            "display": {"theme": "dark"},
            "sessionList": {"width": "wide", "accountNames": true, "sort": "activity"},
        }));
        let effects = reduce(&mut app, ListShapeAction::CycleSort);
        let Effect::SaveSettings { patch } = &effects[0] else { panic!("expected a write") };
        assert_eq!(patch["sessionList"]["sort"], "created", "ours is updated");
        assert!(patch.get("display").is_none(), "a section we do not own is not sent");
        assert!(
            patch["sessionList"].get("width").is_none(),
            "a sibling key we do not own is not sent"
        );
        let blob = app.settings_blob.as_ref().expect("known");
        assert_eq!(blob["display"]["theme"], "dark", "the local copy still has it");
        assert_eq!(blob["sessionList"]["width"], "wide");
        assert_eq!(blob["sessionList"]["sort"], "created");
    }

    /// The F3 case for this writer: with no row read, the write is still a
    /// patch of our own keys, not a full replace built from an empty map.
    #[test]
    fn a_write_with_no_settings_row_read_is_still_only_a_patch() {
        let mut app = app();
        assert_eq!(app.settings_blob, None);
        let effects = reduce(&mut app, ListShapeAction::CycleSort);
        let Effect::SaveSettings { patch } = &effects[0] else { panic!("expected a write") };
        let keys: Vec<&String> = patch.as_object().expect("an object").keys().collect();
        assert_eq!(keys, vec!["sessionList"], "nothing outside what we own");
        assert_eq!(app.settings_blob, None, "an unread row stays unread");
    }

    #[test]
    fn a_blob_the_server_never_sent_still_writes_our_keys() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::CycleGroupBy);
        let list = saved_list(&effects);
        assert_eq!(list["groupBy"], "label");
        assert_eq!(list["section"], "starred,live,dispatched");
    }

    #[test]
    fn the_selection_comes_back_inside_a_list_that_just_shrank() {
        let mut app = app();
        app.selected_index = 1;
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        reduce(&mut app, ListShapeAction::SectionsNext);
        reduce(&mut app, ListShapeAction::SectionsToggle);
        assert_eq!(app.selected_index, 0, "an index past the end would panic a view");
    }

    #[test]
    fn the_popup_only_opens_over_the_list() {
        let mut app = app();
        app.router.push(crate::app::View::Conversation);
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        assert!(app.sections_menu.is_none());
    }
}
