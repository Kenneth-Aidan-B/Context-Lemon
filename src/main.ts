import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

interface Config {
  folders: string[];
  lemonade_url: string;
  chat_model: string;
}

// One generation model Lemonade has installed that fits under the memory cap.
interface ChatModel {
  id: string;
  size_gb: number;
  estimated_ram_gb: number;
  light: boolean;
  max_context_window: number | null;
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

interface IndexProgressDetail {
  folder: string;
  files_done: number;
  files_total: number;
  chunks_indexed: number;
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
const answerText = document.querySelector<HTMLDivElement>("#answer-text")!;
const sourcesBox = document.querySelector<HTMLDivElement>("#sources-box")!;
const sourcesList = document.querySelector<HTMLUListElement>("#sources-list")!;
const modelSelect = document.querySelector<HTMLSelectElement>("#model-select")!;
const modelHintEl = document.querySelector<HTMLParagraphElement>("#model-hint")!;

function appendInlineMarkup(parent: HTMLElement, text: string) {
  // Render only the two inline forms the local models use reliably. Building DOM
  // nodes instead of assigning innerHTML keeps model output inert and safe.
  const pattern = /(\*\*.+?\*\*|`[^`]+`)/g;
  let cursor = 0;
  for (const match of text.matchAll(pattern)) {
    const index = match.index ?? 0;
    if (index > cursor) parent.append(document.createTextNode(text.slice(cursor, index)));

    const token = match[0];
    const element = token.startsWith("**")
      ? document.createElement("strong")
      : document.createElement("code");
    element.textContent = token.startsWith("**") ? token.slice(2, -2) : token.slice(1, -1);
    parent.append(element);
    cursor = index + token.length;
  }
  if (cursor < text.length) parent.append(document.createTextNode(text.slice(cursor)));
}

function renderAnswer(markdown: string) {
  answerText.replaceChildren();

  const lines = markdown.replace(/\r\n/g, "\n").split("\n");
  let paragraphLines: string[] = [];
  let activeList: HTMLOListElement | HTMLUListElement | null = null;
  let codeLines: string[] | null = null;
  let codeLanguage = "";

  const flushParagraph = () => {
    if (paragraphLines.length === 0) return;
    const paragraph = document.createElement("p");
    appendInlineMarkup(paragraph, paragraphLines.join(" "));
    answerText.append(paragraph);
    paragraphLines = [];
  };

  const flushCode = () => {
    if (codeLines === null) return;
    const pre = document.createElement("pre");
    const code = document.createElement("code");
    if (codeLanguage) code.dataset.language = codeLanguage;
    code.textContent = codeLines.join("\n");
    pre.append(code);
    answerText.append(pre);
    codeLines = null;
    codeLanguage = "";
  };

  for (const rawLine of lines) {
    const trimmed = rawLine.trim();
    const fence = trimmed.match(/^```([^`]*)$/);
    if (fence) {
      if (codeLines === null) {
        flushParagraph();
        activeList = null;
        codeLines = [];
        codeLanguage = fence[1].trim();
      } else {
        flushCode();
      }
      continue;
    }
    if (codeLines !== null) {
      codeLines.push(rawLine);
      continue;
    }

    if (!trimmed) {
      flushParagraph();
      activeList = null;
      continue;
    }

    const heading = trimmed.match(/^(#{1,3})\s+(.+)$/);
    if (heading) {
      flushParagraph();
      activeList = null;
      const tagName = heading[1].length === 1 ? "h3" : heading[1].length === 2 ? "h4" : "h5";
      const element = document.createElement(tagName);
      appendInlineMarkup(element, heading[2]);
      answerText.append(element);
      continue;
    }

    const orderedItem = trimmed.match(/^\d+\.\s+(.+)$/);
    const bulletItem = trimmed.match(/^[-*]\s+(.+)$/);
    if (orderedItem || bulletItem) {
      flushParagraph();
      const needsOrderedList = Boolean(orderedItem);
      if (
        !activeList ||
        (needsOrderedList && activeList.tagName !== "OL") ||
        (!needsOrderedList && activeList.tagName !== "UL")
      ) {
        activeList = document.createElement(needsOrderedList ? "ol" : "ul");
        answerText.append(activeList);
      }
      const item = document.createElement("li");
      appendInlineMarkup(item, (orderedItem ?? bulletItem)![1]);
      activeList.append(item);
      continue;
    }

    activeList = null;
    paragraphLines.push(trimmed);
  }

  flushParagraph();
  flushCode();
}

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

// Grouped by whether a model leaves the machine comfortably usable, so "I want this
// running in the background all day" is a choice you can make by reading one label
// rather than by comparing gigabyte figures.
function renderModelOptions(models: ChatModel[], selected: string) {
  modelSelect.replaceChildren();

  if (models.length === 0) {
    const option = document.createElement("option");
    option.textContent = "No installed model fits the 6 GB limit";
    modelSelect.append(option);
    modelSelect.disabled = true;
    return;
  }

  const groups: [string, ChatModel[]][] = [
    ["Runs light — under 2 GB", models.filter((m) => m.light)],
    // "up to", not "under": the cap itself is inclusive, so a model estimated at
    // exactly 6 GB is offered and the label has to say so.
    ["Needs more room — up to 6 GB", models.filter((m) => !m.light)],
  ];
  for (const [label, group] of groups) {
    if (group.length === 0) continue;
    const optgroup = document.createElement("optgroup");
    optgroup.label = label;
    for (const model of group) {
      const option = document.createElement("option");
      option.value = model.id;
      option.textContent = `${model.id} — ~${model.estimated_ram_gb.toFixed(2)} GB while running`;
      optgroup.append(option);
    }
    modelSelect.append(optgroup);
  }

  // A model saved in config can disappear if it is uninstalled from Lemonade. Showing
  // it as a dead entry beats silently answering with something the user did not pick.
  if (!models.some((m) => m.id === selected)) {
    const option = document.createElement("option");
    option.value = selected;
    option.textContent = `${selected} — not installed`;
    option.disabled = true;
    modelSelect.prepend(option);
  }
  modelSelect.value = selected;
  modelSelect.disabled = false;
}

function renderModelHint(models: ChatModel[], selected: string) {
  const model = models.find((m) => m.id === selected);
  if (!model) {
    modelHintEl.textContent = `${selected} is no longer installed in Lemonade — pick another model.`;
    return;
  }
  const context = model.max_context_window
    ? ` · ${Math.round(model.max_context_window / 1024)}K context`
    : "";
  const light = models.filter((m) => m.light).length;
  modelHintEl.textContent =
    `${model.size_gb.toFixed(2)} GB on disk · ~${model.estimated_ram_gb.toFixed(2)} GB while running` +
    `${context}. ${light} of ${models.length} installed model(s) stay under 2 GB.`;
}

async function refreshModels() {
  try {
    const [models, config] = await Promise.all([
      invoke<ChatModel[]>("list_chat_models"),
      invoke<Config>("get_config"),
    ]);
    renderModelOptions(models, config.chat_model);
    renderModelHint(models, config.chat_model);
  } catch (err) {
    modelSelect.replaceChildren();
    modelSelect.disabled = true;
    modelHintEl.textContent = `Could not list models: ${err}`;
  }
}

modelSelect.addEventListener("change", async () => {
  const model = modelSelect.value;
  modelSelect.disabled = true;
  modelHintEl.textContent = `Switching to ${model}…`;
  try {
    await invoke<Config>("set_chat_model", { model });
    await refreshModels();
    // Lemonade loads a newly selected model lazily, and a cold load of an 8B checkpoint
    // measured ~38s here. Without saying so, that first wait sits behind an unexplained
    // "Thinking…" and reads as the app having hung.
    modelHintEl.textContent =
      `Switched to ${model}. The first answer may take longer while Lemonade loads it.`;
  } catch (err) {
    // Re-read first so a rejected switch snaps the dropdown back to the model actually
    // in use, and only then report why — refreshing afterwards would overwrite it.
    await refreshModels();
    modelHintEl.textContent = `Could not switch model: ${err}`;
  }
});

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

listen<IndexProgressDetail>("index-progress-detail", (event) => {
  const p = event.payload;
  const remaining = Math.max(p.files_total - p.files_done, 0);
  if (p.files_total === 0) {
    indexStatusEl.textContent = "Indexing… scanning folder";
    return;
  }
  indexStatusEl.textContent =
    `Indexing… ${p.files_done}/${p.files_total} file(s) scanned · ` +
    `${remaining} remaining · ${p.chunks_indexed} chunk(s) embedded`;
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
  if (askSubmit.disabled) return;
  const question = askInput.value.trim();
  if (!question) return;
  askInput.value = "";

  askSubmit.disabled = true;
  askSubmit.textContent = "Thinking…";
  answerBox.hidden = false;
  answerText.replaceChildren();
  sourcesList.replaceChildren();
  sourcesBox.hidden = true;

  try {
    const response = await invoke<AskResponse>("ask_question", { question });
    renderAnswer(response.answer);
    for (const source of response.sources) {
      const li = document.createElement("li");
      const file = document.createElement("code");
      file.textContent = source.file;
      const location = document.createElement("span");
      location.textContent = `lines ${source.start_line}-${source.end_line}`;
      li.append(file, location);
      sourcesList.appendChild(li);
    }
    sourcesBox.hidden = response.sources.length === 0;
  } catch (err) {
    renderAnswer(`**Error:** ${err}`);
  } finally {
    askSubmit.disabled = false;
    askSubmit.textContent = "Ask";
    askInput.focus();
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
refreshModels();
