use bootloader_api::info::FrameBufferInfo;
use conquer_once::spin::OnceCell;
use core::fmt::Write;
use spin::Mutex;

use crate::frame_buffer::FrameBufferWriter;

pub static LOGGER: OnceCell<LockedLogger> = OnceCell::uninit();

pub struct LockedLogger {
    frame_buf_writer: Option<Mutex<FrameBufferWriter>>,
}

impl LockedLogger {
    pub fn new(frame_buffer: &'static mut [u8], info: FrameBufferInfo) -> Self {
        LockedLogger {
            frame_buf_writer: Some(Mutex::new(FrameBufferWriter::new(frame_buffer, info))),
        }
    }

    pub unsafe fn force_unlock(&self) {
        if let Some(frame_buffer) = &self.frame_buf_writer {
            unsafe { frame_buffer.force_unlock() };
        }
    }
}

impl log::Log for LockedLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if let Some(framebuffer) = &self.frame_buf_writer {
            let mut frame_buffer = framebuffer.lock();
            writeln!(frame_buffer, "{:5}: {}", record.level(), record.args()).unwrap();
        }
    }

    fn flush(&self) {}
}

#[macro_export]
macro_rules! println {
    () => (log::info!());
    ($($arg:tt)*) => (log::info!("{}", format_args!($($arg)*)));
}
