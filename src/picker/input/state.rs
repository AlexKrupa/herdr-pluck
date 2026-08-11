use super::PickerInputEvent;

/// Result of feeding one key into the fixed-width hint state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InputDecision {
    Continue,
    Cancel,
    CopyHint(String),
    OpenHint(String),
    InvalidHint,
}

/// Pure fixed-width hint buffer for picker input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InputState {
    width: usize,
    buffer: String,
    open_requested: bool,
}

impl InputState {
    pub(crate) fn new(width: usize) -> Self {
        Self {
            width,
            buffer: String::new(),
            open_requested: false,
        }
    }

    pub(crate) fn push(&mut self, event: PickerInputEvent, valid_hints: &[&str]) -> InputDecision {
        match event {
            PickerInputEvent::Escape | PickerInputEvent::CtrlC => InputDecision::Cancel,
            PickerInputEvent::Enter | PickerInputEvent::Other => InputDecision::Continue,
            PickerInputEvent::Char(ch) => {
                if self.width == 0 {
                    return InputDecision::Continue;
                }

                self.open_requested |= ch.is_ascii_uppercase();
                self.buffer.push(ch.to_ascii_lowercase());
                if self.buffer.chars().count() < self.width {
                    return InputDecision::Continue;
                }

                let entered = std::mem::take(&mut self.buffer);
                let open = std::mem::take(&mut self.open_requested);
                if !valid_hints.iter().any(|hint| *hint == entered) {
                    InputDecision::InvalidHint
                } else if open {
                    InputDecision::OpenHint(entered)
                } else {
                    InputDecision::CopyHint(entered)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_one_character_hint_requests_copy() {
        let mut state = InputState::new(1);

        assert_eq!(
            state.push(PickerInputEvent::Char('a'), &["a"]),
            InputDecision::CopyHint("a".to_string())
        );
    }

    #[test]
    fn exact_two_character_hint_waits_for_full_width() {
        let mut state = InputState::new(2);

        assert_eq!(
            state.push(PickerInputEvent::Char('a'), &["as"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('s'), &["as"]),
            InputDecision::CopyHint("as".to_string())
        );
    }

    #[test]
    fn invalid_full_width_hint_clears_buffer() {
        let mut state = InputState::new(2);

        assert_eq!(
            state.push(PickerInputEvent::Char('a'), &["sd"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('x'), &["sd"]),
            InputDecision::InvalidHint
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('s'), &["sd"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('d'), &["sd"]),
            InputDecision::CopyHint("sd".to_string())
        );
    }

    #[test]
    fn uppercase_hint_requests_open() {
        let mut state = InputState::new(1);

        assert_eq!(
            state.push(PickerInputEvent::Char('A'), &["a"]),
            InputDecision::OpenHint("a".to_string())
        );
    }

    #[test]
    fn one_uppercase_char_makes_the_whole_hint_open() {
        let mut state = InputState::new(2);

        assert_eq!(
            state.push(PickerInputEvent::Char('a'), &["as"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('S'), &["as"]),
            InputDecision::OpenHint("as".to_string())
        );
    }

    #[test]
    fn invalid_hint_clears_the_open_flag() {
        let mut state = InputState::new(2);

        assert_eq!(
            state.push(PickerInputEvent::Char('A'), &["as"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('x'), &["as"]),
            InputDecision::InvalidHint
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('a'), &["as"]),
            InputDecision::Continue
        );
        assert_eq!(
            state.push(PickerInputEvent::Char('s'), &["as"]),
            InputDecision::CopyHint("as".to_string())
        );
    }

    #[test]
    fn escape_and_ctrl_c_cancel() {
        let mut state = InputState::new(1);
        assert_eq!(
            state.push(PickerInputEvent::Escape, &[]),
            InputDecision::Cancel
        );
        assert_eq!(
            state.push(PickerInputEvent::CtrlC, &[]),
            InputDecision::Cancel
        );
    }

    #[test]
    fn enter_is_ignored() {
        let mut state = InputState::new(1);
        assert_eq!(
            state.push(PickerInputEvent::Enter, &["a"]),
            InputDecision::Continue
        );
    }
}
