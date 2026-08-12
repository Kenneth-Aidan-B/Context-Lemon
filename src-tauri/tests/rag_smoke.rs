use lemonade_context_engine_lib::indexing::store::VectorStore;
use lemonade_context_engine_lib::lemonade::LemonadeClient;
use lemonade_context_engine_lib::rag;

fn index_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap()
        .join("lemonade-context-engine")
        .join("index.bin")
}

/// Requires a running Lemonade server (localhost:13305) with the sample folder
/// already indexed by the app (see README "Instant demo"). Skips instead of
/// failing when either precondition isn't met, since this exercises live
/// infrastructure rather than pure logic.
#[tokio::test]
async fn known_answer_questions_are_grounded_with_citations() {
    let client = LemonadeClient::new("http://localhost:13305/v1".to_string());
    if !client.is_reachable().await {
        eprintln!("SKIPPED: Lemonade server not reachable at localhost:13305");
        return;
    }

    let path = index_path();
    if !path.exists() {
        eprintln!("SKIPPED: no index found at {path:?} — run the app once first");
        return;
    }
    let store = VectorStore::open(path);

    let cases = [
        ("What port does the Nightingale gateway listen on by default?", "7913"),
        ("Who is the lead engineer on Project Nightingale?", "Priya"),
        ("How long can a device be offline before Nightingale falls back to cloud inference?", "90"),
    ];

    for (question, expected_fact) in cases {
        let response = rag::ask(&store, &client, question)
            .await
            .unwrap_or_else(|e| panic!("ask() failed for {question:?}: {e}"));

        assert!(
            !response.sources.is_empty(),
            "expected at least one citation for {question:?}, got none"
        );
        assert!(
            response.answer.contains(expected_fact),
            "expected answer to {question:?} to mention {expected_fact:?}, got: {}",
            response.answer
        );
    }
}
