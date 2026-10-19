//! Shared, bounded target selection for shortcuts and live workspace previews.
use crate::state::Villain;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Direction {
    Next,
    Previous,
}
impl Direction {
    pub fn incoming_offset(self, width: i32) -> i32 {
        match self {
            Self::Next => width,
            Self::Previous => -width,
        }
    }
}

fn target(
    occupied: &[bool],
    source: usize,
    direction: Direction,
    skip_empty: bool,
) -> Option<usize> {
    let count = occupied.len();
    if source >= count {
        return None;
    }
    for distance in 1..count {
        let index = match direction {
            Direction::Next => (source + distance) % count,
            Direction::Previous => (source + count - distance) % count,
        };
        if !skip_empty || occupied[index] {
            return Some(index);
        }
    }
    None
}

impl Villain {
    pub(crate) fn workspace_target(&self, source: usize, direction: Direction) -> Option<usize> {
        let occupied = self
            .workspaces
            .each_ref()
            .map(|workspace| !workspace.is_empty());
        target(
            &occupied,
            source,
            direction,
            self.config.workspace.skip_empty,
        )
    }
    pub(crate) fn workspace_occupied(&self, index: usize) -> bool {
        self.workspaces
            .get(index)
            .is_some_and(|workspace| !workspace.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sparse_slots_skip_and_wrap_in_both_directions() {
        let occupied = [
            true, false, false, true, false, false, true, false, false, false,
        ];
        for (source, next, previous) in [(0, 3, 6), (3, 6, 0), (6, 0, 3)] {
            assert_eq!(target(&occupied, source, Direction::Next, true), Some(next));
            assert_eq!(
                target(&occupied, source, Direction::Previous, true),
                Some(previous)
            );
        }
        assert_eq!(target(&occupied, 1, Direction::Next, true), Some(3));
        assert_eq!(target(&occupied, 1, Direction::Previous, true), Some(0));
    }
    #[test]
    fn disabled_policy_visits_empty_slots_and_wraps() {
        assert_eq!(target(&[false; 10], 0, Direction::Next, false), Some(1));
        assert_eq!(target(&[false; 10], 0, Direction::Previous, false), Some(9));
        assert_eq!(target(&[true; 10], 9, Direction::Next, true), Some(0));
    }
    #[test]
    fn nowhere_else_to_go_is_a_no_op() {
        for direction in [Direction::Next, Direction::Previous] {
            assert_eq!(target(&[false; 10], 0, direction, true), None);
            assert_eq!(target(&[true, false, false], 0, direction, true), None);
            assert_eq!(target(&[true, false, false], 2, direction, true), Some(0));
            assert_eq!(target(&[true], 0, direction, false), None);
            assert_eq!(target(&[], 0, direction, true), None);
            assert_eq!(target(&[true], 9, direction, true), None);
        }
    }
    #[test]
    fn slide_side_depends_on_travel_not_workspace_number() {
        assert_eq!(Direction::Next.incoming_offset(1280), 1280);
        assert_eq!(Direction::Previous.incoming_offset(1280), -1280);
    }
}
