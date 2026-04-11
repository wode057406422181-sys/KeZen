use tokio::sync::{broadcast, mpsc};

use crate::constants::engine::{ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER};
use crate::engine::events::{EngineEvent, UserAction};

/// 父子 Agent 间通信的 channel 对。
///
/// 每个父子关系对应一对 channel：
///   - 父→子：`mpsc<UserAction>` — 父节点向子节点发送指令（单生产者多消费者）
///   - 子→父：`broadcast<EngineEvent>` — 子节点产生的事件广播给父节点及其他订阅者
///
/// ```text
///   Parent (Master/Gateway)
///     │                     ▲
///     │ action_tx ──────►   │ event_tx.subscribe()
///     ▼                     │
///   Child (Worker/Master)
///     action_rx             event_tx
/// ```
pub struct ChannelPair {
    // ── 父端持有（用于向子节点发送指令） ──
    /// 父→子 action sender。克隆此 sender 可以让多个上游同时向子节点发指令。
    pub action_tx: mpsc::Sender<UserAction>,

    // ── 子端持有（用于接收来自父节点的指令） ──
    /// 父→子 action receiver。传给子节点的 KezenEngine 或 AgentNode。
    pub action_rx: Option<mpsc::Receiver<UserAction>>,

    // ── 子端持有（用于广播事件） ──
    /// 子→父 event broadcast sender。子节点持有，每产生一个事件就 `send()`。
    /// 父节点通过 `event_tx.subscribe()` 获取 receiver。
    pub event_tx: broadcast::Sender<EngineEvent>,
}

impl ChannelPair {
    /// 从 channel pair 中取走 `action_rx`（只能 take 一次）。
    ///
    /// 用于将 receiver 端传给子节点的 KezenEngine 构造函数——
    /// KezenEngine 需要 `mpsc::Receiver<UserAction>` 的所有权。
    pub fn take_action_rx(&mut self) -> Option<mpsc::Receiver<UserAction>> {
        self.action_rx.take()
    }

    /// 创建一个 event broadcast 的新订阅者。
    ///
    /// 父节点通过此方法订阅子节点产生的事件流。
    /// 可以调用多次——每次返回一个独立的 receiver。
    pub fn subscribe_events(&self) -> broadcast::Receiver<EngineEvent> {
        self.event_tx.subscribe()
    }
}

/// 创建一对 Agent 间通信 channel。
///
/// # Arguments
/// - `action_buffer` — 父→子 MPSC channel 的容量
/// - `event_buffer` — 子→父 broadcast channel 的容量
///
/// # Returns
/// 一个 `ChannelPair`，持有所有 channel 端。
pub fn create_agent_channel_pair(action_buffer: usize, event_buffer: usize) -> ChannelPair {
    let (action_tx, action_rx) = mpsc::channel(action_buffer);
    let (event_tx, _) = broadcast::channel(event_buffer);

    ChannelPair {
        action_tx,
        action_rx: Some(action_rx),
        event_tx,
    }
}

/// 使用默认 buffer 大小创建一对 Agent 间通信 channel。
///
/// 默认值：
/// - action buffer = `ACTION_CHANNEL_BUFFER` (32)
/// - event buffer = `EVENT_CHANNEL_BUFFER` (64)
pub fn create_default_channel_pair() -> ChannelPair {
    create_agent_channel_pair(ACTION_CHANNEL_BUFFER, EVENT_CHANNEL_BUFFER)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Channel 消息传递正确性 ───────────────────────────────────────────

    #[tokio::test]
    async fn action_channel_send_receive() {
        let mut pair = create_default_channel_pair();
        let rx = pair.take_action_rx().unwrap();

        pair.action_tx
            .send(UserAction::SendMessage {
                content: "hello".to_string(),
            })
            .await
            .unwrap();

        // Use a oneshot-style receive to avoid blocking forever
        let mut rx = rx;
        let msg = rx.recv().await.unwrap();
        assert_eq!(
            msg,
            UserAction::SendMessage {
                content: "hello".to_string()
            }
        );
    }

    #[tokio::test]
    async fn event_broadcast_send_receive() {
        let pair = create_default_channel_pair();
        let mut rx = pair.subscribe_events();

        let _ = pair.event_tx.send(EngineEvent::Done);

        let evt = rx.recv().await.unwrap();
        assert!(matches!(evt, EngineEvent::Done));
    }

    // ── Channel 关闭检测 ────────────────────────────────────────────────

    #[tokio::test]
    async fn action_channel_sender_drop_closes_receiver() {
        let mut pair = create_default_channel_pair();
        let mut rx = pair.take_action_rx().unwrap();

        // Drop the sender：子节点的 receiver 应该收到 None。
        drop(pair.action_tx);

        let result = rx.recv().await;
        assert!(
            result.is_none(),
            "Receiver should get None when sender is dropped"
        );
    }

    #[tokio::test]
    async fn event_broadcast_sender_drop_closes_receiver() {
        let pair = create_default_channel_pair();
        let mut rx = pair.subscribe_events();

        // Drop the sender
        drop(pair.event_tx);

        let result = rx.recv().await;
        assert!(
            result.is_err(),
            "Broadcast receiver should get Err when sender is dropped"
        );
    }

    // ── Broadcast 多订阅者同时收到消息 ──────────────────────────────────

    #[tokio::test]
    async fn broadcast_multiple_subscribers() {
        let pair = create_default_channel_pair();

        let mut rx1 = pair.subscribe_events();
        let mut rx2 = pair.subscribe_events();
        let mut rx3 = pair.subscribe_events();

        // Send one event
        let _ = pair.event_tx.send(EngineEvent::TextDelta {
            text: "shared".to_string(),
        });

        // All three subscribers should receive it
        let e1 = rx1.recv().await.unwrap();
        let e2 = rx2.recv().await.unwrap();
        let e3 = rx3.recv().await.unwrap();

        assert!(matches!(e1, EngineEvent::TextDelta { text } if text == "shared"));
        assert!(matches!(e2, EngineEvent::TextDelta { text } if text == "shared"));
        assert!(matches!(e3, EngineEvent::TextDelta { text } if text == "shared"));
    }

    // ── take_action_rx 只能 take 一次 ──────────────────────────────────

    #[test]
    fn take_action_rx_returns_none_on_second_call() {
        let mut pair = create_default_channel_pair();
        assert!(pair.take_action_rx().is_some());
        assert!(pair.take_action_rx().is_none());
    }

    // ── 自定义 buffer 大小 ──────────────────────────────────────────────

    #[tokio::test]
    async fn custom_buffer_sizes() {
        let mut pair = create_agent_channel_pair(4, 8);
        let mut rx = pair.take_action_rx().unwrap();

        // Fill up the action buffer (capacity 4)
        for i in 0..4 {
            pair.action_tx
                .send(UserAction::SendMessage {
                    content: format!("msg-{}", i),
                })
                .await
                .unwrap();
        }

        // Drain and verify order
        for i in 0..4 {
            let msg = rx.recv().await.unwrap();
            assert_eq!(
                msg,
                UserAction::SendMessage {
                    content: format!("msg-{}", i)
                }
            );
        }
    }
}
