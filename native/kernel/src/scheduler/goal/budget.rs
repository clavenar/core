use clv_abi::budget::ReplyBudget;

pub(super) struct ReplyAccount {
    limit: Option<usize>,
    // Only an unlimited goal can delegate an unlimited reservation.
    reserved: Option<usize>,
    spent: usize,
}

impl ReplyAccount {
    pub(super) fn new(limit: Option<usize>) -> Self {
        Self {
            limit,
            reserved: Some(0),
            spent: 0,
        }
    }

    pub(super) fn snapshot(&self) -> ReplyBudget {
        ReplyBudget {
            available: self
                .limit
                .map(|limit| limit - self.spent - self.reserved.unwrap_or(0)),
            reserved: self.reserved,
            spent: self.spent,
        }
    }

    pub(super) fn reserve(&mut self, requested: Option<usize>) -> Option<()> {
        let available = self.snapshot().available;
        if let (Some(bytes), Some(available)) = (requested, available)
            && bytes > available
        {
            return None;
        }
        self.reserved = requested.or(available);
        Some(())
    }

    pub(super) fn charge(&mut self, bytes: usize) -> Option<()> {
        if let Some(remaining) = &mut self.reserved {
            *remaining = remaining.checked_sub(bytes)?;
        }
        // An unlimited goal can outlive the range of its reporting counter.
        self.spent = self.spent.saturating_add(bytes);
        Some(())
    }

    pub(super) fn release(&mut self) {
        self.reserved = Some(0);
    }
}
