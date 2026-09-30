use super::state::View;

/// A never-empty stack of views. Overlays (help, dialogs) push; dismissing
/// pops back to whatever was underneath.
#[derive(Debug, Clone)]
pub struct Router {
    stack: Vec<View>,
}

impl Router {
    pub fn new(root: View) -> Self {
        Self { stack: vec![root] }
    }

    pub fn current(&self) -> View {
        self.stack.last().copied().unwrap_or(View::SessionList)
    }

    pub fn below(&self) -> Option<View> {
        self.stack.len().checked_sub(2).and_then(|i| self.stack.get(i).copied())
    }

    /// Pushing the current view again is a no-op, so a repeated keypress
    /// cannot grow the stack without bound.
    pub fn push(&mut self, view: View) {
        if self.current() != view {
            self.stack.push(view);
        }
    }

    /// `false` at the root, which is never popped.
    pub fn pop(&mut self) -> bool {
        if self.stack.len() <= 1 {
            return false;
        }
        self.stack.pop();
        true
    }

    pub fn reset(&mut self, root: View) {
        self.stack.clear();
        self.stack.push(root);
    }

    #[cfg(test)]
    pub const fn depth(&self) -> usize {
        self.stack.len()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new(View::SessionList)
    }
}

#[cfg(test)]
mod tests {
    use super::{Router, View};

    #[test]
    fn root_is_never_popped() {
        let mut router = Router::new(View::SessionList);
        assert!(!router.pop());
        assert_eq!(router.current(), View::SessionList);
        assert_eq!(router.depth(), 1);
    }

    #[test]
    fn push_pop_restores_the_view_below() {
        let mut router = Router::new(View::SessionList);
        router.push(View::Conversation);
        router.push(View::PermissionDialog);
        assert_eq!(router.current(), View::PermissionDialog);
        assert_eq!(router.below(), Some(View::Conversation));
        assert!(router.pop());
        assert_eq!(router.current(), View::Conversation);
        assert_eq!(router.below(), Some(View::SessionList));
    }

    #[test]
    fn pushing_the_current_view_is_a_noop() {
        let mut router = Router::new(View::SessionList);
        router.push(View::Help);
        router.push(View::Help);
        assert_eq!(router.depth(), 2);
    }

    #[test]
    fn reset_collapses_to_one_view() {
        let mut router = Router::new(View::SessionList);
        router.push(View::Conversation);
        router.push(View::Help);
        router.reset(View::SessionList);
        assert_eq!(router.depth(), 1);
        assert_eq!(router.current(), View::SessionList);
        assert_eq!(router.below(), None);
    }
}
