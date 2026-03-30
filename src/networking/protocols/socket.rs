use core::task::{Context, Poll, Waker};

use alloc::{collections::vec_deque::VecDeque, vec::Vec};
use spin::Mutex;

use crate::{networking::protocols::ip::IPProtocol, println};

pub static SOCKET_TABLE: Mutex<SocketTable> = Mutex::new(SocketTable {
    entries: Vec::new(),
});

pub struct SocketTable {
    entries: Vec<SocketEntry>,
}

struct SocketEntry {
    src_port: u16,
    protocol: IPProtocol,
    waker: Option<Waker>,
    incomming: VecDeque<Vec<u8>>,
}

impl SocketTable {
    pub fn bind(&mut self, src_port: u16, protocol: IPProtocol) -> Result<(), &'static str> {
        if self
            .entries
            .iter()
            .any(|e| e.src_port == src_port && e.protocol == protocol)
        {
            return Err("Port already in use!");
        }
        self.entries.push(SocketEntry {
            src_port,
            protocol,
            incomming: VecDeque::new(),
            waker: None,
        });
        Ok(())
    }

    pub fn unbind(&mut self, port: u16, protocol: IPProtocol) {
        self.entries
            .retain(|e| !(e.src_port == port && e.protocol == protocol));
    }

    pub fn deliver(&mut self, port: u16, protocol: IPProtocol, data: Vec<u8>) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.src_port == port && e.protocol == protocol)
        {
            entry.incomming.push_back(data);
            if let Some(waker) = entry.waker.take() {
                waker.wake();
            }
        }
    }

    pub fn poll_recv(
        &mut self,
        port: u16,
        protocol: IPProtocol,
        cx: &mut Context,
    ) -> Poll<Vec<u8>> {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| e.src_port == port && e.protocol == protocol)
        {
            if let Some(data) = entry.incomming.pop_front() {
                return Poll::Ready(data);
            }
            entry.waker = Some(cx.waker().clone());
        }
        Poll::Pending
    }
}

pub struct RecvPacket {
    pub port: u16,
    pub protocol: IPProtocol,
}

impl Future for RecvPacket {
    type Output = Vec<u8>;

    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        SOCKET_TABLE.lock().poll_recv(self.port, self.protocol, cx)
    }
}
