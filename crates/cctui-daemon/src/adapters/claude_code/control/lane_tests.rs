use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::sync::Notify;

use super::test_support::*;
use super::*;

type Ops = Arc<Mutex<Vec<(String, String, String)>>>;

const BOUND: Duration = Duration::from_secs(5);

/// A `reply` to `held` waits for `release`; a worker is alive until killed.
fn fake_daemon(d: &Driver, held: &str, release: Arc<Notify>) -> Ops {
    let dir = d.cfg.jobs_root.parent().unwrap().join("daemon").join("00000000");
    std::fs::create_dir_all(&dir).unwrap();
    let listener = tokio::net::UnixListener::bind(dir.join("control.sock")).unwrap();
    let ops = Ops::default();
    let seen = Arc::clone(&ops);
    let held = held.to_owned();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else { return };
            let (seen, held, release) = (Arc::clone(&seen), held.clone(), Arc::clone(&release));
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                if tokio::io::BufReader::new(read).read_line(&mut line).await.is_err() {
                    return;
                }
                let req: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
                let field = |k: &str| req[k].as_str().unwrap_or_default().to_owned();
                let (op, short) = (field("op"), field("short"));
                let killed = {
                    let mut seen = seen.lock().unwrap();
                    let killed = seen.iter().any(|(o, s, _)| o == "kill" && *s == short);
                    seen.push((op.clone(), short.clone(), field("text")));
                    killed
                };
                if op == "reply" && short == held {
                    release.notified().await;
                }
                let answer = json!({ "ok": true, "alive": !killed, "jobs": [] });
                let _ = write.write_all(format!("{answer}\n").as_bytes()).await;
            });
        }
    });
    ops
}

/// Mid-turn, so a reply skips the composer clear and the Enter.
fn live(d: &mut Driver, short: &str) -> String {
    let local_id = format!("{short}-0000-4000-8000-000000000000");
    d.short_by_session.insert(local_id.clone(), short.to_owned());
    d.roster.insert(short.to_owned());
    let status = StatusSnapshot {
        tempo: Some("active".into()),
        state: Some("working".into()),
        detail: None,
        name: None,
        activity: None,
        model: None,
        effort: None,
        source: None,
        started_at: None,
        asking: false,
    };
    d.last_status.insert(short.to_owned(), status);
    local_id
}

fn reply(local_id: &str, text: &str) -> (AdapterCommand, uuid::Uuid) {
    let id = uuid::Uuid::new_v4();
    let cmd = AdapterCommand::Reply {
        local_id: local_id.to_owned(),
        recover_auth: false,
        text: text.to_owned(),
        ask_picks: None,
        env: BTreeMap::new(),
        command_id: Some(id),
        turn_id: None,
    };
    (cmd, id)
}

async fn command_result(rx: &mut mpsc::Receiver<AdapterEvent>, id: uuid::Uuid) -> bool {
    tokio::time::timeout(BOUND, async {
        loop {
            match rx.recv().await.expect("event stream open") {
                AdapterEvent::CommandResult { command_id, ok, .. } if command_id == id => {
                    return ok;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the command reports within the bound")
}

fn ops_for(ops: &Ops, short: &str) -> Vec<(String, String)> {
    let ops = ops.lock().unwrap();
    ops.iter().filter(|(_, s, _)| s == short).map(|(o, _, t)| (o.clone(), t.clone())).collect()
}

async fn until_op(ops: &Ops, short: &str, op: &str, count: usize) {
    tokio::time::timeout(BOUND, async {
        while ops_for(ops, short).iter().filter(|(o, _)| o == op).count() < count {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{short} never saw {count} `{op}`: {:?}", ops_for(ops, short)));
}

async fn finish_next_lane(d: &mut Driver) {
    let done = tokio::time::timeout(BOUND, d.lanes.done_rx.recv())
        .await
        .expect("the lane frees within the bound")
        .expect("lane channel open");
    let events = d.events.clone();
    d.finish_lane(&events, done).await;
}

#[tokio::test]
async fn a_slow_reply_to_one_session_holds_up_no_other_session_and_no_poll() {
    let (mut d, mut rx) = driver();
    let release = Arc::new(Notify::new());
    let ops = fake_daemon(&d, "a1a1a1a1", Arc::clone(&release));
    let slow = live(&mut d, "a1a1a1a1");
    let other = live(&mut d, "b2b2b2b2");
    let events = d.events.clone();

    let (cmd, slow_id) = reply(&slow, "slow");
    tokio::time::timeout(BOUND, d.route_command(&events, cmd))
        .await
        .expect("a reply only enqueues its delivery");
    until_op(&ops, "a1a1a1a1", "reply", 1).await;

    let (cmd, other_id) = reply(&other, "meanwhile");
    tokio::time::timeout(BOUND, d.route_command(&events, cmd)).await.unwrap();
    assert!(command_result(&mut rx, other_id).await, "another session is delivered meanwhile");
    let _ = tokio::time::timeout(BOUND, d.poll_once()).await.expect("the poll runs meanwhile");
    assert!(d.lanes.is_busy(&slow), "the slow reply is still in flight");

    release.notify_one();
    assert!(command_result(&mut rx, slow_id).await);
}

#[tokio::test]
async fn two_replies_to_one_session_are_delivered_in_order() {
    let (mut d, mut rx) = driver();
    let release = Arc::new(Notify::new());
    let ops = fake_daemon(&d, "c3c3c3c3", Arc::clone(&release));
    let session = live(&mut d, "c3c3c3c3");
    let events = d.events.clone();

    let (first, first_id) = reply(&session, "first");
    let (second, second_id) = reply(&session, "second");
    d.route_command(&events, first).await;
    d.route_command(&events, second).await;
    assert_eq!(d.lanes.queued(&session), 1, "the second reply waits behind the first");
    until_op(&ops, "c3c3c3c3", "reply", 1).await;

    release.notify_one();
    assert!(command_result(&mut rx, first_id).await);
    finish_next_lane(&mut d).await;
    until_op(&ops, "c3c3c3c3", "reply", 2).await;
    release.notify_one();
    assert!(command_result(&mut rx, second_id).await);

    let texts: Vec<String> = ops_for(&ops, "c3c3c3c3")
        .into_iter()
        .filter(|(op, _)| op == "reply")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(texts, ["first", "second"]);
}

#[tokio::test]
async fn a_remove_racing_a_reply_on_one_session_runs_after_the_reply() {
    let (mut d, mut rx) = driver();
    d.cfg.claude_bin = "/nonexistent/claude-bin".to_owned();
    let release = Arc::new(Notify::new());
    let ops = fake_daemon(&d, "d4d4d4d4", Arc::clone(&release));
    let session = live(&mut d, "d4d4d4d4");
    let events = d.events.clone();

    let (cmd, reply_id) = reply(&session, "last words");
    d.route_command(&events, cmd).await;
    let remove_id = uuid::Uuid::new_v4();
    let remove = AdapterCommand::Remove {
        local_id: session.clone(),
        command_id: Some(remove_id),
        initiator: RemoveInitiator::User,
    };
    d.route_command(&events, remove).await;
    assert_eq!(d.lanes.queued(&session), 1, "the remove waits behind the reply");
    until_op(&ops, "d4d4d4d4", "reply", 1).await;

    release.notify_one();
    assert!(command_result(&mut rx, reply_id).await);
    finish_next_lane(&mut d).await;
    assert!(!command_result(&mut rx, remove_id).await, "the test has no `claude` to rm with");

    let order: Vec<String> = ops_for(&ops, "d4d4d4d4").into_iter().map(|(op, _)| op).collect();
    let at = |op: &str| order.iter().position(|o| o == op).unwrap_or_else(|| panic!("{order:?}"));
    assert!(at("reply") < at("kill"), "the worker is killed only after the reply: {order:?}");
}

#[tokio::test(start_paused = true)]
async fn the_reseed_skips_sessions_pulled_within_the_interval_and_runs_off_the_loop() {
    let (mut d, _rx) = driver();
    let fresh = live(&mut d, "e0e0e0e0");
    let stale: Vec<String> = (0..8).map(|n| live(&mut d, &format!("f000000{n}"))).collect();
    d.pulls.note(&fresh);
    let interval = Duration::from_hours(1);

    let mut targets = d.reseed_targets(interval);
    targets.sort();
    assert_eq!(targets, stale, "a session pulled at launch is not pulled again");

    let started = tokio::time::Instant::now();
    let task = reseed::spawn_reseed(targets, d.pulls.clone(), |_| async {
        tokio::time::sleep(Duration::from_secs(1)).await;
        anyhow::Ok(true)
    });
    assert!(!task.is_finished(), "the pass runs on its own task");
    assert_eq!(task.await.unwrap(), 8);
    let took = started.elapsed();
    assert!(
        took >= Duration::from_secs(2) && took < Duration::from_secs(3),
        "eight 1s pulls, four at a time: {took:?}"
    );
    assert!(d.reseed_targets(interval).is_empty(), "every pulled session is fresh now");
}
