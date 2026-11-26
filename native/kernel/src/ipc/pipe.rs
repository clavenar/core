use clv_abi::PIPE_CAPACITY;

pub(crate) enum PipeResult {
    Ready(usize),
    Wait,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Reader {
    Open,
    Reading,
    Closed,
}

pub(crate) struct Pipe {
    bytes: [u8; PIPE_CAPACITY],
    length: usize,
    reader: Reader,
    writer_open: bool,
}

impl Pipe {
    pub(crate) fn new() -> Self {
        Self {
            bytes: [0; PIPE_CAPACITY],
            length: 0,
            reader: Reader::Open,
            writer_open: true,
        }
    }

    pub(crate) fn read(&mut self, capacity: usize, copy: impl FnOnce(&[u8]) -> bool) -> PipeResult {
        if self.reader == Reader::Closed {
            return PipeResult::Error;
        }
        if capacity == 0 {
            return PipeResult::Ready(0);
        }
        if self.length == 0 {
            if self.writer_open {
                return PipeResult::Wait;
            }
            self.reader = Reader::Open;
            return PipeResult::Ready(0);
        }
        let count = capacity.min(self.length);
        if !copy(&self.bytes[..count]) {
            return PipeResult::Error;
        }
        self.reader = Reader::Reading;
        self.bytes.copy_within(count..self.length, 0);
        self.length -= count;
        PipeResult::Ready(count)
    }

    pub(crate) fn write(
        &mut self,
        length: usize,
        copy: impl FnOnce(&mut [u8]) -> bool,
    ) -> PipeResult {
        if self.reader == Reader::Closed || !self.writer_open {
            return PipeResult::Error;
        }
        let count = length.min(PIPE_CAPACITY - self.length);
        if length > 0 && count == 0 {
            return PipeResult::Wait;
        }
        let end = self.length + count;
        if !copy(&mut self.bytes[self.length..end]) {
            return PipeResult::Error;
        }
        self.length = end;
        PipeResult::Ready(count)
    }

    pub(crate) fn close_reader(&mut self) -> bool {
        self.length = 0;
        core::mem::replace(&mut self.reader, Reader::Closed) != Reader::Closed
    }

    pub(crate) fn close_writer(&mut self) -> bool {
        core::mem::replace(&mut self.writer_open, false)
    }

    pub(crate) fn is_reading(&self) -> bool {
        self.reader == Reader::Reading
    }

    pub(crate) fn reader_open(&self) -> bool {
        self.reader != Reader::Closed
    }
}
