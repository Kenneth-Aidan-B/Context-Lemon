import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Config {
  folders: string[];
  lemonade_url: string;
}

interface IndexStatus {
  files: number;
  chunks: number;
  resident_bytes: number;
}

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface IndexStats {
  files_indexed: number;
  chunks_indexed: number;
  files_skipped_unchanged: number;
  files_purged: number;
  files_failed: number;
  cancelled: boolean;
}

interface Source {
  file: string;
  start_line: number;
  end_line: number;
}

interface AskResponse {
  answer: string;
  sources: Source[];
}

const folderListEl = document.querySelector<HTMLUListElement>("#folder-list")!;
const addFolderBtn = document.querySelector<HTMLButtonElement>("#add-folder-btn")!;
const indexStatusEl = document.querySelector<HTMLParagraphElement>("#index-status")!;
const statusEl = document.querySelector<HTMLParagraphElement>("#status-msg")!;
const askForm = document.querySelector<HTMLFormElement>("#ask-form")!;
const askInput = document.querySelector<HTMLInputElement>("#ask-input")!;
const askSubmit = document.querySelector<HTMLButtonElement>("#ask-submit")!;
const answerBox = document.querySelector<HTMLDivElement>("#answer-box")!;
const answerText = document.querySelector<HTMLParagraphElement>("#answer-text")!;
const sourcesList = document.querySelector<HTMLUListElement>("#sources-list")!;

function renderFolders(config: Config) {
  folderListEl.innerHTML = "";
  if (config.folders.length === 0) {
    const li = document.createElement("li");
    li.className = "empty";
    li.textContent = "No folders yet — add one to start indexing.";
    folderListEl.appendChild(li);
    return;
  }
  for (const folder of config.folders) {
    const li = document.createElement("li");
    const span = document.createElement("span");
    span.textContent = folder;
    // The path is ellipsized in the row, so keep the full value reachable on hover.
    span.title = folder;
    const removeBtn = document.createElement("button");
    removeBtn.textContent = "Remove";
    removeBtn.className = "remove-btn";
    removeBtn.addEventListener("click", async () => {
      try {
        const updated = await invoke<Config>("remove_folder", { folder });
        renderFolders(updated);
        refreshIndexStatus();
      } catch (err) {
        indexStatusEl.textContent = `Could not remove folder: ${err}`;
      }
    });
    li.appendChild(span);
    li.appendChild(removeBtn);
    folderListEl.appendChild(li);
  }
}

async function refreshConfig() {
  try {
    const config = await invoke<Config>("get_config");
    renderFolders(config);
  } catch (err) {
    indexStatusEl.textContent = `Could not load config: ${err}`;
  }
}

async function refreshIndexStatus() {
  try {
    const status = await invoke<IndexStatus>("get_index_status");
    indexStatusEl.textContent =
      `${status.files} file(s) indexed · ${status.chunks} chunk(s) · ` +
      `${formatBytes(status.resident_bytes)} RAM`;
  } catch (err) {
    indexStatusEl.textContent = `Could not read index status: ${err}`;
  }
}

// A corrupt or version-mismatched index otherwise looks identical to "nothing
// indexed yet", so surface it explicitly.
async function reportIndexLoadError() {
  try {
    const err = await invoke<string | null>("take_index_load_error");
    if (err) indexStatusEl.textContent = err;
  } catch {
    /* non-fatal */
  }
}

addFolderBtn.addEventListener("click", () => {
  invoke("add_folder_dialog").catch((err) => {
    indexStatusEl.textContent = `Could not open folder picker: ${err}`;
  });
});

listen("folders-updated", () => {
  refreshConfig();
});

listen<string>("index-progress", (event) => {
  indexStatusEl.textContent = `Indexing ${event.payload}…`;
});

listen("index-updated", () => {
  refreshIndexStatus();
});

listen<IndexStats>("index-done", (event) => {
  const s = event.payload;
  if (s.cancelled) {
    // The folder was removed (or re-added) mid-run; the partial result is expected.
    refreshIndexStatus();
    return;
  }
  const parts = [`Indexed ${s.files_indexed} file(s) · ${s.chunks_indexed} chunk(s)`];
  if (s.files_skipped_unchanged > 0) parts.push(`${s.files_skipped_unchanged} unchanged`);
  if (s.files_purged > 0) parts.push(`${s.files_purged} removed`);
  if (s.files_failed > 0) parts.push(`${s.files_failed} failed`);
  indexStatusEl.textContent = parts.join(" · ");
});

listen<string>("index-error", (event) => {
  indexStatusEl.textContent = `Indexing failed: ${event.payload}`;
});

askForm.addEventListener("submit", async (e) => {
  e.preventDefault();
  const question = askInput.value.trim();
  if (!question) return;

  askSubmit.disabled = true;
  askSubmit.textContent = "Thinking…";
  answerBox.hidden = false;
  answerText.textContent = "";
  sourcesList.innerHTML = "";

  try {
    const response = await invoke<AskResponse>("ask_question", { question });
    answerText.textContent = response.answer;
    for (const source of response.sources) {
      const li = document.createElement("li");
      li.textContent = `${source.file} (lines ${source.start_line}-${source.end_line})`;
      sourcesList.appendChild(li);
    }
  } catch (err) {
    answerText.textContent = `Error: ${err}`;
  } finally {
    askSubmit.disabled = false;
    askSubmit.textContent = "Ask";
  }
});

async function checkLemonade() {
  try {
    const reachable = await invoke<boolean>("check_lemonade");
    statusEl.textContent = reachable
      ? "Connected to Lemonade server."
      : "Lemonade server not reachable at localhost:13305.";
  } catch {
    statusEl.textContent = "Lemonade server not reachable at localhost:13305.";
  }
}

refreshConfig();
refreshIndexStatus().then(reportIndexLoadError);
checkLemonade();
