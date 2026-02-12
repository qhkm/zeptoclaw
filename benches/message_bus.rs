//! Message Bus Benchmarks
//!
//! Run with: cargo bench --bench message_bus

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::sync::Arc;
use tokio::runtime::Runtime;
use zeptoclaw::bus::{InboundMessage, MessageBus};

fn benchmark_publish_inbound(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("message_bus");
    group.throughput(Throughput::Elements(1));

    group.bench_function("publish_inbound", |b| {
        b.to_async(&rt).iter(|| async {
            let bus = MessageBus::new();
            let msg = InboundMessage::new("test", "user1", "chat1", "Hello world");
            bus.publish_inbound(black_box(msg)).await.unwrap();
        });
    });

    group.bench_function("publish_consume_roundtrip", |b| {
        b.to_async(&rt).iter(|| async {
            let bus = MessageBus::new();
            let msg = InboundMessage::new("test", "user1", "chat1", "Hello world");
            bus.publish_inbound(msg).await.unwrap();
            let _ = bus.consume_inbound().await;
        });
    });

    group.finish();
}

fn benchmark_concurrent_publish(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();

    let mut group = c.benchmark_group("concurrent_publish");

    for num_messages in [100, 1000, 10000].iter() {
        group.throughput(Throughput::Elements(*num_messages as u64));
        group.bench_with_input(
            format!("{}_messages", num_messages),
            num_messages,
            |b, &n| {
                b.to_async(&rt).iter(|| async {
                    let bus = Arc::new(MessageBus::new());
                    let bus2 = Arc::clone(&bus);

                    // Spawn consumer
                    let consumer = tokio::spawn(async move {
                        let mut count = 0;
                        while count < n {
                            if bus2.consume_inbound().await.is_some() {
                                count += 1;
                            }
                        }
                    });

                    // Publish messages
                    for i in 0..n {
                        let msg =
                            InboundMessage::new("test", &format!("user{}", i), "chat1", "Hello");
                        bus.publish_inbound(msg).await.unwrap();
                    }

                    consumer.await.unwrap();
                });
            },
        );
    }

    group.finish();
}

criterion_group!(
    benches,
    benchmark_publish_inbound,
    benchmark_concurrent_publish
);
criterion_main!(benches);
