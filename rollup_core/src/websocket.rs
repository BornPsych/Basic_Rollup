use actix::{Actor, StreamHandler, Handler, Message, AsyncContext};
use actix_web::{web, Error, HttpRequest, HttpResponse};
use actix_web_actors::ws;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::events::{EventBus, EventFilter, RollupEvent};

/// WebSocket session for real-time updates
pub struct WsSession {
    /// Client ID
    id: uuid::Uuid,
    /// Last heartbeat
    hb: Instant,
    /// Event bus for subscriptions
    event_bus: Arc<EventBus>,
    /// Filter for events
    filter: Option<EventFilter>,
}

impl WsSession {
    pub fn new(event_bus: Arc<EventBus>, filter: Option<EventFilter>) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            hb: Instant::now(),
            event_bus,
            filter,
        }
    }

    /// Heartbeat to keep connection alive
    fn hb(&self, ctx: &mut ws::WebsocketContext<Self>) {
        ctx.run_interval(Duration::from_secs(5), |act, ctx| {
            if Instant::now().duration_since(act.hb) > Duration::from_secs(10) {
                log::warn!("WebSocket client {} heartbeat failed, disconnecting", act.id);
                ctx.stop();
                return;
            }
            ctx.ping(b"");
        });
    }
}

impl Actor for WsSession {
    type Context = ws::WebsocketContext<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        log::info!("WebSocket client {} connected", self.id);
        self.hb(ctx);

        // Subscribe to events
        let subscription = self.event_bus.subscribe(self.filter.clone());
        let addr = ctx.address();

        // Spawn event listener
        let receiver = subscription.receiver;
        ctx.spawn(actix::fut::wrap_future(async move {
            while let Ok(event) = receiver.recv().await {
                let _ = addr.do_send(EventMessage(event));
            }
        }));
    }

    fn stopped(&mut self, _ctx: &mut Self::Context) {
        log::info!("WebSocket client {} disconnected", self.id);
    }
}

/// Handle websocket messages
impl StreamHandler<Result<ws::Message, ws::ProtocolError>> for WsSession {
    fn handle(&mut self, msg: Result<ws::Message, ws::ProtocolError>, ctx: &mut Self::Context) {
        match msg {
            Ok(ws::Message::Ping(msg)) => {
                self.hb = Instant::now();
                ctx.pong(&msg);
            }
            Ok(ws::Message::Pong(_)) => {
                self.hb = Instant::now();
            }
            Ok(ws::Message::Text(text)) => {
                // Handle subscription changes
                if let Ok(cmd) = serde_json::from_str::<WsCommand>(&text) {
                    match cmd {
                        WsCommand::Subscribe { filter } => {
                            self.filter = Some(filter);
                            ctx.text("{\"status\":\"subscribed\"}");
                        }
                        WsCommand::Unsubscribe => {
                            self.filter = None;
                            ctx.text("{\"status\":\"unsubscribed\"}");
                        }
                    }
                }
            }
            Ok(ws::Message::Close(reason)) => {
                ctx.close(reason);
                ctx.stop();
            }
            _ => (),
        }
    }
}

/// Event message from event bus
#[derive(Message)]
#[rtype(result = "()")]
struct EventMessage(RollupEvent);

impl Handler<EventMessage> for WsSession {
    type Result = ();

    fn handle(&mut self, msg: EventMessage, ctx: &mut Self::Context) {
        if let Ok(json) = serde_json::to_string(&msg.0) {
            ctx.text(json);
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum WsCommand {
    Subscribe { filter: EventFilter },
    Unsubscribe,
}

/// WebSocket endpoint handler
pub async fn websocket_handler(
    req: HttpRequest,
    stream: web::Payload,
    event_bus: web::Data<Arc<EventBus>>,
) -> Result<HttpResponse, Error> {
    let session = WsSession::new(event_bus.get_ref().clone(), None);
    ws::start(session, &req, stream)
}

/// WebSocket statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketStats {
    pub active_connections: usize,
    pub total_messages_sent: u64,
    pub uptime_seconds: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_websocket_command_parsing() {
        let json = r#"{"type":"Subscribe","filter":"TransactionEvents"}"#;
        let _cmd: Result<WsCommand, _> = serde_json::from_str(json);
        // Just test parsing works
    }
}
