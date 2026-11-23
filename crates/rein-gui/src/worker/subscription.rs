// Author: Onogwu Franklin Kelechi (kelexine) <https://github.com/kelexine>
// Date: 2026-11-23
// Purpose: Iced Subscription polling worker events only when busy or woken, plus global shortcuts

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use iced::futures::Stream;
use iced::{Subscription, keyboard, stream};

use crate::app::Message;

#[derive(Clone)]
struct PollFlags {
    busy: Arc<AtomicBool>,
    wake: Arc<AtomicBool>,
}

impl Hash for PollFlags {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Arc::as_ptr(&self.busy).hash(state);
        Arc::as_ptr(&self.wake).hash(state);
    }
}

/// Emits `Message::Tick` only when busy or when background worker events are pending, plus `Ctrl+B` and `Esc`.
pub fn app_subscription(
    busy_flag: Arc<AtomicBool>,
    wake_flag: Arc<AtomicBool>,
) -> Subscription<Message> {
    let tick = Subscription::run_with(
        PollFlags {
            busy: busy_flag,
            wake: wake_flag,
        },
        |flags| tick_stream(Arc::clone(&flags.busy), Arc::clone(&flags.wake)),
    );
    let keys = keyboard::listen().filter_map(|event| match event {
        keyboard::Event::KeyPressed { key, modifiers, .. } => {
            if modifiers.control()
                && matches!(key, keyboard::Key::Character(ref c) if c.eq_ignore_ascii_case("b"))
            {
                return Some(Message::ToggleSidebar);
            }
            if matches!(key, keyboard::Key::Named(keyboard::key::Named::Escape)) {
                return Some(Message::CancelTurn);
            }
            None
        }
        _ => None,
    });
    Subscription::batch([tick, keys])
}

fn tick_stream(
    busy_flag: Arc<AtomicBool>,
    wake_flag: Arc<AtomicBool>,
) -> impl Stream<Item = Message> {
    stream::channel(
        4,
        move |mut tx: iced::futures::channel::mpsc::Sender<Message>| async move {
            std::thread::spawn(move || {
                let mut last_idle_check = Instant::now();
                loop {
                    std::thread::sleep(Duration::from_millis(60));
                    let is_busy = busy_flag.load(Ordering::Relaxed);
                    let has_wake = wake_flag.swap(false, Ordering::AcqRel);
                    let idle_task_poll = last_idle_check.elapsed() >= Duration::from_millis(500);
                    if is_busy || has_wake || idle_task_poll {
                        if idle_task_poll {
                            last_idle_check = Instant::now();
                        }
                        if let Err(err) = tx.try_send(Message::Tick)
                            && err.is_disconnected()
                        {
                            break;
                        }
                    }
                }
            });
            std::future::pending::<()>().await;
        },
    )
}
