use clv_abi::budget::ReplyBudget;

pub(super) struct ReplyReservation {
    remaining: Option<usize>,
    spent: usize,
}

impl ReplyReservation {
    pub(super) fn new(available: Option<usize>, requested: Option<usize>) -> Option<Self> {
        if let (Some(bytes), Some(available)) = (requested, available)
            && bytes > available
        {
            return None;
        }
        Some(Self {
            remaining: requested.or(available),
            spent: 0,
        })
    }

    /// Compute both shares so the caller can commit them after acquiring any file grant.
    pub(super) fn split(&self, bytes: usize) -> Option<(Self, Self)> {
        let helper = Self::new(self.remaining, Some(bytes))?;
        let parent = Self {
            remaining: self.remaining.map(|remaining| remaining - bytes),
            spent: self.spent,
        };
        Some((parent, helper))
    }

    pub(super) fn remaining(&self) -> Option<usize> {
        self.remaining
    }

    pub(super) fn charge(&mut self, bytes: usize) -> Option<()> {
        if let Some(remaining) = &mut self.remaining {
            *remaining = remaining.checked_sub(bytes)?;
        }
        // An unlimited goal can outlive the range of its reporting counter.
        self.spent = self.spent.saturating_add(bytes);
        Some(())
    }

    pub(super) fn release(&mut self) -> Option<usize> {
        self.remaining.replace(0)
    }

    pub(super) fn refund(&mut self, bytes: usize) {
        if let Some(remaining) = &mut self.remaining {
            // The helper can return at most what this reservation delegated.
            *remaining += bytes;
        }
    }
}

pub(super) fn balance<'a>(
    limit: Option<usize>,
    reservations: impl Iterator<Item = &'a ReplyReservation>,
) -> ReplyBudget {
    let mut reserved = Some(0usize);
    let mut spent = 0usize;
    for reservation in reservations {
        reserved = reserved
            .zip(reservation.remaining)
            .map(|(total, remaining)| total.saturating_add(remaining));
        spent = spent.saturating_add(reservation.spent);
    }
    // A finite goal admits only reservations that fit, so both totals are exact.
    ReplyBudget {
        available: limit.map(|limit| limit - spent - reserved.unwrap_or(0)),
        reserved,
        spent,
    }
}
