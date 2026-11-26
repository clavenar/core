/// Reply payload bytes held by a goal, reserved by its live children, and spent.
/// An unlimited balance is represented by `None`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReplyBudget {
    pub available: Option<usize>,
    pub reserved: Option<usize>,
    pub spent: usize,
}

impl ReplyBudget {
    pub fn registers(self) -> (usize, usize, usize, usize) {
        let flags =
            usize::from(self.available.is_none()) | (usize::from(self.reserved.is_none()) << 1);
        (
            flags,
            self.available.unwrap_or(0),
            self.reserved.unwrap_or(0),
            self.spent,
        )
    }

    pub fn from_registers(
        flags: usize,
        available: usize,
        reserved: usize,
        spent: usize,
    ) -> Option<Self> {
        let (available, reserved) = match flags {
            0 => (Some(available), Some(reserved)),
            1 => (None, Some(reserved)),
            3 => (None, None),
            _ => return None,
        };
        Some(Self {
            available,
            reserved,
            spent,
        })
    }
}
