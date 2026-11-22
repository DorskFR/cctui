use super::*;

fn status_of(e: &AppError) -> StatusCode {
    match e {
        AppError::Status(s, _) => *s,
        other => panic!("expected a status error, got {other:?}"),
    }
}

const ENOENT: &str = "cannot resolve /nope: No such file or directory (os error 2)";

fn lazy_state() -> AppState {
    AppState::for_test(sqlx::PgPool::connect_lazy("postgres://invalid").unwrap())
}

#[tokio::test]
async fn dir_missing_reads_the_daemons_listing() {
    let state = lazy_state();
    let machine = Uuid::new_v4();
    assert!(!dir_missing(&state, machine, "/w").await, "an offline machine is not a miss");

    let daemon = answer_list_dirs(&state, machine, Err(ENOENT.into()));
    assert!(dir_missing(&state, machine, "/nope").await);
    daemon.await.unwrap();

    let daemon = answer_list_dirs(&state, machine, Ok(vec!["src".into()]));
    assert!(!dir_missing(&state, machine, "/w").await);
    daemon.await.unwrap();

    let outside = "/etc is outside the allowed roots";
    let daemon = answer_list_dirs(&state, machine, Err(outside.into()));
    assert!(!dir_missing(&state, machine, "/etc").await);
    daemon.await.unwrap();
}

#[tokio::test]
async fn listing_a_missing_dir_answers_404() {
    let state = lazy_state();
    let machine = Uuid::new_v4();
    let daemon = answer_list_dirs(&state, machine, Err(ENOENT.into()));
    let err = list_dirs(
        State(state.clone()),
        Path(machine.to_string()),
        Query(ListDirsParams { path: "/nope".into() }),
    )
    .await
    .unwrap_err();
    daemon.await.unwrap();
    assert_eq!(status_of(&err), StatusCode::NOT_FOUND);

    let offline = list_dirs(
        State(state),
        Path(Uuid::new_v4().to_string()),
        Query(ListDirsParams { path: "/w".into() }),
    )
    .await
    .unwrap_err();
    assert_eq!(status_of(&offline), StatusCode::SERVICE_UNAVAILABLE);
}
