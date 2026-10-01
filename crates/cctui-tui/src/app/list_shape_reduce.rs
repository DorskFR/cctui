//! The keys that change the list's shape: `f` sections, `o`/`O` sort, `v`
//! group-by. Every change is written back to the server settings the web UI
//! reads, so the two agree on the next reload.

use super::action::Effect;
use super::list_view::{SECTIONS, ShapeKey, next_sort};
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
            settle(app, &[ShapeKey::Section])
        }
        ListShapeAction::CycleSort => {
            let chosen = app.list_shape.sort.next();
            let (sort, dir) = next_sort(app.list_shape.sort, app.list_shape.sort_dir, chosen);
            app.list_shape.sort = sort;
            app.list_shape.sort_dir = dir;
            // A new field takes its natural direction, so both keys moved.
            settle(app, &[ShapeKey::Sort, ShapeKey::SortDir])
        }
        ListShapeAction::FlipSortDir => {
            app.list_shape.sort_dir = app.list_shape.sort_dir.flipped();
            settle(app, &[ShapeKey::SortDir])
        }
        ListShapeAction::CycleGroupBy => {
            app.list_shape.group_by = app.list_shape.group_by.next();
            settle(app, &[ShapeKey::GroupBy])
        }
        ListShapeAction::CycleColorBy => {
            app.list_shape.color_by = next_color_by(app.list_shape.color_by);
            settle(app, &[ShapeKey::ColorBy])
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
fn settle(app: &mut App, changed: &[ShapeKey]) -> Vec<Effect> {
    app.reshape();
    vec![save(app, changed)]
}

/// Sends the keys this action changed, and nothing else.
fn save(app: &App, changed: &[ShapeKey]) -> Effect {
    super::settings_write::save(
        serde_json::json!({"sessionList": app.list_shape.settings_patch_for(changed)}),
    )
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
    }

    /// R11: a write must name only the key the user changed. Re-sending the
    /// other four from the startup snapshot reverts whatever the web UI stored
    /// for them since.
    #[test]
    fn changing_the_sort_does_not_resend_the_other_shape_keys() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::CycleSort);
        let list = saved_list(&effects);
        let mut keys: Vec<&str> =
            list.as_object().expect("an object").keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec!["sort", "sortDir"],
            "only the field and the direction it brought with it"
        );
    }

    /// One case per action, so a key cannot creep back into a patch that does
    /// not own it.
    #[test]
    fn every_shape_action_writes_exactly_the_keys_it_changed() {
        let cases: &[(ListShapeAction, &[&str])] = &[
            (ListShapeAction::FlipSortDir, &["sortDir"]),
            (ListShapeAction::CycleGroupBy, &["groupBy"]),
            (ListShapeAction::CycleColorBy, &["colorBy"]),
            (ListShapeAction::CycleSort, &["sort", "sortDir"]),
        ];
        for (action, expected) in cases {
            let mut app = app();
            let effects = reduce(&mut app, *action);
            let list = saved_list(&effects);
            let mut keys: Vec<&str> =
                list.as_object().expect("an object").keys().map(String::as_str).collect();
            keys.sort_unstable();
            let mut want = expected.to_vec();
            want.sort_unstable();
            assert_eq!(keys, want, "{action:?} wrote the wrong key set");
        }

        let mut app = app();
        reduce(&mut app, ListShapeAction::ToggleSectionsMenu);
        let effects = reduce(&mut app, ListShapeAction::SectionsToggle);
        let list = saved_list(&effects);
        let keys: Vec<&str> =
            list.as_object().expect("an object").keys().map(String::as_str).collect();
        assert_eq!(keys, vec!["section"], "toggling a section writes only the section key");
    }

    /// The other half of R11: the in-memory shape is re-read from what the
    /// server actually stored, so a sibling key the web UI changed is adopted
    /// rather than staying stale until restart.
    #[test]
    fn a_confirmed_write_adopts_the_servers_value_for_a_key_the_web_ui_changed() {
        let mut app = app();
        assert_eq!(app.list_shape.color_by, ColorBy::None);
        crate::app::reduce(
            &mut app,
            crate::app::Action::SettingsSaved(Box::new(serde_json::json!({
                "sessionList": {"sort": "created", "sortDir": "desc", "colorBy": "label"},
            }))),
        );
        assert_eq!(app.list_shape.sort, Sort::Created, "our own change came back");
        assert_eq!(
            app.list_shape.color_by,
            ColorBy::Label,
            "the web UI's change to a key we did not touch is adopted"
        );
    }

    /// The write was refused, so the server still holds the old shape; showing
    /// the new one would be a lie (R15, the F3 residual).
    #[test]
    fn a_refused_write_puts_the_shape_back_to_what_the_server_holds() {
        let mut app = app();
        app.settings_blob = Some(serde_json::json!({
            "sessionList": {"sort": "activity", "sortDir": "desc", "groupBy": "status"},
        }));
        reduce(&mut app, ListShapeAction::CycleSort);
        assert_eq!(app.list_shape.sort, Sort::Created, "shown optimistically");

        crate::app::reduce(&mut app, crate::app::Action::SettingsWriteFailed);
        assert_eq!(
            app.list_shape.sort,
            Sort::Activity,
            "nothing was stored, so the shape goes back"
        );
    }

    /// With no row ever read there is nothing truer to fall back to, so the
    /// user keeps what they just chose rather than being reset to defaults.
    #[test]
    fn a_refused_write_with_no_row_read_keeps_what_the_user_chose() {
        let mut app = app();
        assert_eq!(app.settings_blob, None);
        reduce(&mut app, ListShapeAction::CycleGroupBy);
        let chosen = app.list_shape.group_by;
        crate::app::reduce(&mut app, crate::app::Action::SettingsWriteFailed);
        assert_eq!(app.list_shape.group_by, chosen);
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

    /// A write is not a confirmation: until the server answers, the cached row
    /// must keep the value the server actually holds.
    #[test]
    fn a_pending_write_does_not_move_the_cached_row() {
        let mut app = app();
        app.settings_blob =
            Some(serde_json::json!({"sessionList": {"sort": "activity"}, "theme": "dark"}));
        let before = app.settings_blob.clone();
        reduce(&mut app, ListShapeAction::CycleSort);
        assert_eq!(app.settings_blob, before, "nothing is cached before the server confirms");
    }

    #[test]
    fn a_blob_the_server_never_sent_still_writes_the_changed_key() {
        let mut app = app();
        let effects = reduce(&mut app, ListShapeAction::CycleGroupBy);
        let list = saved_list(&effects);
        assert_eq!(list["groupBy"], "label");
        assert!(
            list.get("section").is_none(),
            "an unread row is no reason to write a default over the web UI's value"
        );
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
