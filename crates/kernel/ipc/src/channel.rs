//! RPC channel — combined send+recv for request/reply (v0.20.0).
//!
//! Provides [`call`], which performs a synchronous RPC: send a request
//! message to an endpoint, then immediately receive the reply on the same
//! endpoint.

use crate::endpoint::{recv, send, EndpointId, IpcError, Message};

/// Perform a synchronous RPC call on `ep`.
///
/// Sends `req` to the endpoint, then blocks waiting for a reply. This is
/// equivalent to `send(ep, req)?; recv(ep)`.
///
/// Returns `Ok(reply)` on success, or `Err(InvalidEndpoint)` if the
/// endpoint does not exist.
pub fn call(ep: EndpointId, req: &Message) -> Result<Message, IpcError> {
    send(ep, req)?;
    recv(ep)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use eneros_sched::{set_current_tid, Tid};

    use super::*;
    use crate::endpoint::endpoint_create;

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn test_call_invalid_endpoint() {
        let _g = lock();
        let req = Message::default();
        let result = call(EndpointId(999), &req);
        assert_eq!(result, Err(IpcError::InvalidEndpoint));
    }

    #[test]
    fn test_call_equivalent_to_send_recv() {
        let _g = lock();

        let ep = endpoint_create();
        assert_ne!(ep, EndpointId(0));

        // Set up: a "server" is waiting to receive on this endpoint.
        // Calling recv (with no waiting sender) sets waiting_receiver
        // and returns the default message on host.
        let server_tid = Tid(10);
        set_current_tid(server_tid);
        let _ = recv(ep);
        // Now waiting_receiver == Some(server_tid) on the endpoint.

        // Client calls: send request, then recv reply.
        let client_tid = Tid(20);
        set_current_tid(client_tid);

        let mut req = Message {
            label: 0xDEAD,
            ..Default::default()
        };
        req.payload[0] = 42;

        let result = call(ep, &req);
        assert!(result.is_ok(), "call should succeed");
        let reply = result.unwrap();

        // send() delivered the request to the waiting receiver (server),
        // copying it into endpoint.msg. Then recv() (with no waiting
        // sender) returned endpoint.msg — which is the request.
        assert_eq!(
            reply.label, 0xDEAD,
            "reply should contain the request label"
        );
        assert_eq!(
            reply.payload[0], 42,
            "reply should contain the request payload"
        );

        set_current_tid(Tid(0));
    }
}
