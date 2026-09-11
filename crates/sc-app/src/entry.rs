//! Typing an exact number during a drag.
//!
//! Dragging is how you find a size. Typing is how you state one. Chasing 12.00
//! with a pointer is a game, and losing it means dragging until the readout
//! happens to agree, which is slower than saying twelve and worse than saying
//! twelve, because it usually ends at 11.98.
//!
//! So a gesture already in flight takes digits: the numbers go into a buffer,
//! the readout shows what has been typed so far, and Enter commits it. The drag
//! is not interrupted and does not have to be started again, because the moment
//! you know the number is usually a second after you started reaching for it.
//!
//! The buffer is a string rather than a running float on purpose. `1`, `1.`,
//! `1.0` and `1.00` are the same number and four different things to type
//! through, and a float cannot hold the difference between them, so backspace
//! would jump rather than undo a keystroke.

/// A number being typed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Entry {
    text: String,
}

impl Entry {
    /// Takes one character, rejecting anything that would not parse.
    ///
    /// Returns whether it was taken, so the caller can leave a rejected key for
    /// whatever else is listening rather than swallowing it silently.
    pub(crate) fn push(&mut self, c: char) -> bool {
        let ok = match c {
            '0'..='9' => true,
            // A sign only leads, because a minus in the middle of a number is
            // not a smaller number, it is a typing mistake.
            '-' => self.text.is_empty(),
            '.' => !self.text.contains('.'),
            _ => false,
        };
        if ok {
            self.text.push(c);
        }
        ok
    }

    /// Removes the last character. Returns whether anything is left.
    pub(crate) fn backspace(&mut self) -> bool {
        self.text.pop();
        !self.text.is_empty()
    }

    /// What has been typed, for the readout.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The number, if what has been typed is one.
    ///
    /// `-`, `.` and `-.` are all reachable part way through typing a perfectly
    /// good number, so they are not errors, they are simply not a value yet.
    pub(crate) fn value(&self) -> Option<f32> {
        let v: f32 = self.text.parse().ok()?;
        v.is_finite().then_some(v)
    }
}

#[cfg(test)]
mod tests {
    use super::Entry;

    #[test]
    fn it_builds_a_number() {
        let mut e = Entry::default();
        for c in "12.5".chars() {
            assert!(e.push(c), "rejected {c}");
        }
        assert_eq!(e.text(), "12.5");
        assert_eq!(e.value(), Some(12.5));
    }

    /// A negative offset is a real thing to want: moving left is not a
    /// different gesture from moving right.
    #[test]
    fn a_sign_leads_or_is_refused() {
        let mut e = Entry::default();
        assert!(e.push('-'));
        assert!(e.push('4'));
        assert!(!e.push('-'), "took a minus in the middle");
        assert_eq!(e.value(), Some(-4.0));
    }

    #[test]
    fn a_second_point_is_refused() {
        let mut e = Entry::default();
        for c in "1.5".chars() {
            e.push(c);
        }
        assert!(!e.push('.'));
        assert_eq!(e.text(), "1.5");
    }

    #[test]
    fn letters_are_left_alone() {
        let mut e = Entry::default();
        assert!(!e.push('x'), "swallowed a key that means something else");
        assert!(e.text().is_empty());
    }

    /// Backspace has to undo one keystroke, not one order of magnitude. Holding
    /// a float instead would make `1.00` backspace to `1.0` and then to `1`
    /// without the display changing, and `12.` is not a number a float can
    /// hold at all.
    #[test]
    fn backspace_undoes_one_keystroke() {
        let mut e = Entry::default();
        for c in "1.00".chars() {
            e.push(c);
        }
        assert!(e.backspace());
        assert_eq!(e.text(), "1.0");
        assert!(e.backspace());
        assert_eq!(e.text(), "1.");
        assert_eq!(e.value(), Some(1.0), "a trailing point is still one");
        assert!(e.backspace());
        assert_eq!(e.text(), "1");
        assert!(!e.backspace(), "did not report that it emptied");
    }

    /// Part way through typing is not an error, it is just not a number yet.
    #[test]
    fn a_lone_sign_is_not_a_value() {
        let mut e = Entry::default();
        e.push('-');
        assert_eq!(e.value(), None);
        e.push('.');
        assert_eq!(e.value(), None);
        e.push('5');
        assert_eq!(e.value(), Some(-0.5));
    }
}
