use lemonade_context_engine_lib::lemonade::{self, LemonadeClient};
use lemonade_context_engine_lib::rag;
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

mod common;

/// The indexing progress the UI shows ("N/M files scanned · K remaining") is only
/// meaningful if the callback actually fires and its counts are coherent, so assert
/// on the reported sequence rather than trusting that it compiles.
#[tokio::test]
async fn indexing_reports_progress_with_coherent_counts() {
    let Some(fixture) = common::setup().await else {
        return;
    };

    let sample = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../sample");
    let reported: Mutex<Vec<rag::IndexProgress>> = Mutex::new(Vec::new());
    let cancel = AtomicBool::new(false);

    rag::index_folder(
        &fixture.store,
        &fixture.client,
        &sample.to_string_lossy(),
        &cancel,
        &|p| reported.lock().unwrap().push(p),
    )
    .await
    .expect("index_folder failed");

    let reported = reported.into_inner().unwrap();
    assert!(!reported.is_empty(), "no progress was reported at all");

    let last = reported.last().unwrap();
    assert!(last.files_total > 0, "walk reported no files: {last:?}");
    assert_eq!(
        last.files_done, last.files_total,
        "the final progress report must account for every walked file: {last:?}"
    );

    // files_done must never run backwards or overshoot the total, or the UI would
    // show a count that jumps around or a negative "remaining".
    let mut previous = 0usize;
    for p in &reported {
        assert!(
            p.files_done >= previous,
            "files_done went backwards: {previous} then {}",
            p.files_done
        );
        assert!(
            p.files_done <= p.files_total,
            "files_done {} exceeded files_total {}",
            p.files_done,
            p.files_total
        );
        previous = p.files_done;
    }
}

/// Requires a running Lemonade server at localhost:13305. The fixture indexes the
/// bundled sample into disposable storage, keeping the test isolated from the user's
/// application index. It skips only when Lemonade itself is unreachable.
#[tokio::test]
async fn known_answer_questions_are_grounded_with_citations() {
    let Some(fixture) = common::setup().await else {
        return;
    };

    let cases = [
        ("What port does the Nightingale gateway listen on by default?", "7913"),
        ("Who is the lead engineer on Project Nightingale?", "Priya"),
        ("How long can a device be offline before Nightingale falls back to cloud inference?", "90"),
    ];

    for (question, expected_fact) in cases {
        let response = rag::ask(&fixture.store, &fixture.client, question, rag::DEFAULT_CHAT_MODEL)
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

/// The model picker is only as trustworthy as its reading of what Lemonade actually
/// sends, so this drives the real endpoint rather than a fixture. It guards the three
/// promises the UI makes: nothing above the cap is offered, the "runs light" badge
/// means what it says, and the embedding model the indexer depends on is never
/// presented as something you can answer questions with.
#[tokio::test]
async fn installed_chat_models_are_listed_within_the_memory_cap() {
    let client = LemonadeClient::new("http://localhost:13305/v1".to_string());
    if !client.is_reachable().await {
        eprintln!("SKIPPED: Lemonade server not reachable at localhost:13305");
        return;
    }

    let models = client
        .list_chat_models()
        .await
        .expect("listing installed models failed");
    assert!(
        !models.is_empty(),
        "Lemonade is running but offered no chat model at all"
    );

    for model in &models {
        assert!(
            model.estimated_ram_gb <= lemonade::MAX_MODEL_RAM_GB,
            "{} is estimated at {:.2} GB, over the {:.0} GB cap",
            model.id,
            model.estimated_ram_gb,
            lemonade::MAX_MODEL_RAM_GB
        );
        assert_eq!(
            model.light,
            model.estimated_ram_gb < lemonade::LIGHT_MODEL_RAM_GB,
            "the light badge on {} disagrees with its own estimate ({:.2} GB)",
            model.id,
            model.estimated_ram_gb
        );
        assert!(
            !model.id.contains("nomic-embed"),
            "the embedding model must never be selectable for chat"
        );
    }

    // Sorted cheapest-first, so the models that leave the machine usable come first.
    let ordered: Vec<f64> = models.iter().map(|m| m.estimated_ram_gb).collect();
    let mut sorted = ordered.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(ordered, sorted, "models were not ordered by footprint");

    assert!(
        models.iter().any(|m| m.id == rag::DEFAULT_CHAT_MODEL),
        "the default model {} is not installed in Lemonade, so a fresh install of this          app could not answer anything. Got: {:?}",
        rag::DEFAULT_CHAT_MODEL,
        models.iter().map(|m| &m.id).collect::<Vec<_>>()
    );
}
