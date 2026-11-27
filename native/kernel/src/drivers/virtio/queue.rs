use super::{mmio::Device, order_device_accesses};

pub(super) const BUFFER_BYTES: usize = 32;
const QUEUE_SIZE: u32 = 1;
const NO_INTERRUPT: u16 = 1;
const NO_NOTIFY: u16 = 1;
const DEVICE_WRITES: u16 = 2;

#[repr(C, align(16))]
struct Descriptor {
    address: u64,
    length: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
struct Available {
    flags: u16,
    index: u16,
    head: u16,
    used_event: u16,
}

#[repr(C)]
struct UsedElement {
    id: u32,
    length: u32,
}

#[repr(C)]
struct Used {
    flags: u16,
    index: u16,
    element: UsedElement,
    available_event: u16,
}

#[repr(C)]
pub(super) struct QueueMemory {
    descriptor: Descriptor,
    available: Available,
    used: Used,
    buffer: [u8; BUFFER_BYTES],
}

impl QueueMemory {
    pub(super) const fn empty() -> Self {
        Self {
            descriptor: Descriptor {
                address: 0,
                length: 0,
                flags: 0,
                next: 0,
            },
            available: Available {
                flags: NO_INTERRUPT.to_le(),
                index: 0,
                head: 0,
                used_event: 0,
            },
            used: Used {
                flags: 0,
                index: 0,
                element: UsedElement { id: 0, length: 0 },
                available_event: 0,
            },
            buffer: [0; BUFFER_BYTES],
        }
    }
}

pub(super) struct QueueError;

/// A split virtqueue with one descriptor and one fixed-size buffer.
pub(super) struct Queue {
    memory: *mut QueueMemory,
    index: u32,
    completed: u16,
    pending: Option<usize>,
}

impl Queue {
    /// # Safety
    /// The caller must own the queue memory and its identity mapping for the
    /// lifetime of the device's access, and the device must be in reset.
    pub(super) unsafe fn new(memory: *mut QueueMemory, index: u32) -> Self {
        // SAFETY: The reserved storage is idle and exclusively owned by the driver.
        unsafe { memory.write(QueueMemory::empty()) };
        Self {
            memory,
            index,
            completed: 0,
            pending: None,
        }
    }

    pub(super) fn configure(&self, device: &Device) -> bool {
        // SAFETY: These fields are in resident, identity-mapped kernel memory.
        let (descriptor, available, used) = unsafe {
            (
                (&raw const (*self.memory).descriptor).addr() as u64,
                (&raw const (*self.memory).available).addr() as u64,
                (&raw const (*self.memory).used).addr() as u64,
            )
        };
        device.set_queue(self.index, QUEUE_SIZE, descriptor, available, used)
    }

    pub(super) fn offer_receive(&mut self) -> Result<(), QueueError> {
        if self.pending.is_some() {
            return Err(QueueError);
        }
        self.publish(BUFFER_BYTES, DEVICE_WRITES);
        Ok(())
    }

    pub(super) fn offer_send(&mut self, bytes: &[u8]) -> Result<(), QueueError> {
        if self.pending.is_some() || bytes.is_empty() || bytes.len() > BUFFER_BYTES {
            return Err(QueueError);
        }
        // SAFETY: The device has no outstanding descriptor for this buffer.
        unsafe {
            let buffer = (&raw mut (*self.memory).buffer).cast::<u8>();
            for (offset, &byte) in bytes.iter().enumerate() {
                buffer.add(offset).write_volatile(byte);
            }
        }
        self.publish(bytes.len(), 0);
        Ok(())
    }

    fn publish(&mut self, length: usize, flags: u16) {
        self.pending = Some(if flags == DEVICE_WRITES { length } else { 0 });
        // SAFETY: Only the driver writes the descriptor and available ring.
        // Head stays zero because the ring has exactly one descriptor.
        unsafe {
            (&raw mut (*self.memory).descriptor).write_volatile(Descriptor {
                address: ((&raw const (*self.memory).buffer).addr() as u64).to_le(),
                length: (length as u32).to_le(),
                flags: flags.to_le(),
                next: 0,
            });
            order_device_accesses();
            (&raw mut (*self.memory).available.index)
                .write_volatile(self.completed.wrapping_add(1).to_le());
        }
    }

    pub(super) fn notify(&self, device: &Device) {
        // Publish the available index before looking at notification suppression.
        order_device_accesses();
        // SAFETY: The device may update these flags, so read them with a volatile load.
        let flags = u16::from_le(unsafe { (&raw const (*self.memory).used.flags).read_volatile() });
        if flags & NO_NOTIFY == 0 {
            device.notify(self.index);
        }
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn complete(&mut self) -> Result<Option<usize>, QueueError> {
        // SAFETY: The used index is an aligned device-owned field.
        let index = u16::from_le(unsafe { (&raw const (*self.memory).used.index).read_volatile() });
        if index == self.completed {
            return Ok(None);
        }
        let capacity = self.pending.ok_or(QueueError)?;
        if index != self.completed.wrapping_add(1) {
            return Err(QueueError);
        }
        order_device_accesses();
        // SAFETY: The device published the index after writing the used entry.
        let element = unsafe { (&raw const (*self.memory).used.element).read_volatile() };
        let length = u32::from_le(element.length) as usize;
        if u32::from_le(element.id) != 0 || length > capacity {
            return Err(QueueError);
        }
        self.completed = index;
        self.pending = None;
        Ok(Some(length))
    }

    pub(super) fn receive(
        &mut self,
        bytes: &mut [u8; BUFFER_BYTES],
    ) -> Result<Option<usize>, QueueError> {
        let Some(length) = self.complete()? else {
            return Ok(None);
        };
        // SAFETY: Completion transfers the buffer back to the driver and validates length.
        unsafe {
            let buffer = (&raw const (*self.memory).buffer).cast::<u8>();
            for (offset, byte) in bytes[..length].iter_mut().enumerate() {
                *byte = buffer.add(offset).read_volatile();
            }
        }
        Ok(Some(length))
    }
}
