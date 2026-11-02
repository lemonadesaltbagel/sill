//! Async pipe reading utilities for Unix.

use std::io;
use std::os::unix::io::OwnedFd;

use tokio::net::unix::pipe;

pub(crate) struct AsyncPipeReader(pipe::Receiver);

impl AsyncPipeReader {
    pub(crate) fn new(reader: std::io::PipeReader) -> io::Result<Self> {
        Ok(Self(pipe::Receiver::from_file(std::fs::File::from(
            OwnedFd::from(reader),
        ))?))
    }

    /// Reads to the end; output that isn't UTF-8 is decoded, not an error
    /// ([`crate::text::decode_output`]).
    pub(crate) async fn read_to_string(&mut self) -> io::Result<String> {
        use tokio::io::AsyncReadExt;
        let mut bytes = Vec::new();
        self.0.read_to_end(&mut bytes).await?;
        Ok(crate::text::decode_output(&bytes))
    }
}
