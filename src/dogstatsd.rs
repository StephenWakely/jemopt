use tokio::task::yield_now;

use dogstatsd::{Client, Options};
use tokio::time::{Duration, Instant};

pub async fn spam(port: u16, timelimit: Duration) {
    let start = Instant::now();
    
    let to_addr = format!("127.0.0.1:{port}");
    let client = Client::new(Options {
        to_addr,
        ..Options::default()
    }).unwrap();
    let tags = &["nong:wong"];

    loop {
	if start.elapsed() > timelimit {
	    return;
	}

        for i in 0..10000 {
            client.incr(&format!("ziggle.counter{i}"), tags).unwrap();
            client.decr(&format!("ziggle.counter{i}"), tags).unwrap();
            client
                .gauge(&format!("ziggle.guage{i}"), "12345", tags)
                .unwrap();
        }

	yield_now().await;
    }
}
