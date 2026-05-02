const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog, save: saveDialog, confirm } = window.__TAURI__.dialog;

// ── State ───────────────────────────────────────────────────────────────

let groups = [];       // DuplicateGroup[]
let rows = [];         // flat row data for the table
let sortCol = "group";
let sortAsc = true;
let activeFilter = "all";
let selectedRowIdx = -1;
let compareSelection = []; // up to 2 rows for comparison
let undoStack = [];        // [{deletedRows, paths}]

// ── DOM refs ────────────────────────────────────────────────────────────

const btnBrowse = document.getElementById("btn-browse");
const btnScan = document.getElementById("btn-scan");
const btnDeleteAll = document.getElementById("btn-delete-all");
const folderPath = document.getElementById("folder-path");
const chkRecursive = document.getElementById("chk-recursive");
const minWidth = document.getElementById("min-width");
const minHeight = document.getElementById("min-height");
const progressContainer = document.getElementById("progress-container");
const progressText = document.getElementById("progress-text");
const tbody = document.getElementById("results-body");
const emptyState = document.getElementById("empty-state");
const selectAll = document.getElementById("select-all");
const previewContent = document.getElementById("preview-content");
const previewInfo = document.getElementById("preview-info");
const statusText = document.getElementById("status-text");
const statusStats = document.getElementById("status-stats");
const btnExport = document.getElementById("btn-export");
const btnCompare = document.getElementById("btn-compare");
const progressBar = document.getElementById("progress-bar");

// ── Browse folder ───────────────────────────────────────────────────────

btnBrowse.addEventListener("click", async () => {
  try {
    const selected = await openDialog({ directory: true, multiple: false });
    if (selected) {
      folderPath.value = selected;
      btnScan.disabled = false;
    }
  } catch (e) {
    console.error("Dialog error:", e);
  }
});

// ── Scan ────────────────────────────────────────────────────────────────

btnScan.addEventListener("click", async () => {
  const folder = folderPath.value;
  if (!folder) return;

  btnScan.disabled = true;
  btnDeleteAll.disabled = true;
  btnExport.disabled = true;
  progressContainer.classList.remove("hidden");
  progressBar.style.setProperty("--progress", "0%");
  progressText.textContent = "Scanning images...";
  statusText.textContent = "Scanning...";
  emptyState.style.display = "none";
  tbody.textContent = "";

  // Listen for progress events
  const unlistenScan = await listen("scan_progress", (e) => {
    const { scanned, total, current_file } = e.payload;
    const pct = total > 0 ? Math.round((scanned / total) * 100) : 0;
    progressBar.style.setProperty("--progress", pct + "%");
    progressText.textContent = `Scanning: ${scanned}/${total} — ${current_file}`;
  });

  const unlistenDedup = await listen("dedup_progress", (e) => {
    const { done, total, message } = e.payload;
    const pct = total > 0 ? Math.round((done / total) * 100) : 0;
    progressBar.style.setProperty("--progress", pct + "%");
    progressText.textContent = message;
  });

  try {
    // Step 1: scan images
    progressText.textContent = "Scanning images (pHash + MD5)...";
    const images = await invoke("scan_images", {
      folder,
      recursive: chkRecursive.checked,
      minWidth: parseInt(minWidth.value) || 50,
      minHeight: parseInt(minHeight.value) || 50,
    });

    if (images.length === 0) {
      statusText.textContent = "No images found.";
      progressContainer.classList.add("hidden");
      emptyState.style.display = "";
      emptyState.textContent = "No images found in the selected folder.";
      btnScan.disabled = false;
      unlistenScan(); unlistenDedup();
      return;
    }

    progressBar.style.setProperty("--progress", "0%");
    progressText.textContent = `Found ${images.length} images. Running SSIM verification...`;

    // Step 2: find duplicates
    groups = await invoke("find_duplicates", {
      images,
      phashThreshold: 10,
      ssimThreshold: 0.90,
    });

    progressContainer.classList.add("hidden");

    if (groups.length === 0) {
      statusText.textContent = "No duplicates found.";
      emptyState.style.display = "";
      emptyState.textContent = `Scanned ${images.length} images — no duplicates detected.`;
      btnScan.disabled = false;
      unlistenScan(); unlistenDedup();
      return;
    }

    // Build flat row data
    buildRows();
    renderTable();
    updateStats();
    btnDeleteAll.disabled = false;
    btnExport.disabled = false;
    statusText.textContent = "Scan complete.";
  } catch (e) {
    progressContainer.classList.add("hidden");
    statusText.textContent = `Error: ${e}`;
    console.error(e);
  }

  unlistenScan(); unlistenDedup();
  btnScan.disabled = false;
});

// ── Build flat rows from groups ─────────────────────────────────────────

function buildRows() {
  rows = [];
  groups.forEach((group, gi) => {
    const groupNum = gi + 1;

    // Keeper row
    rows.push({
      groupNum,
      groupStart: true,
      action: "KEEP",
      path: group.keeper.path,
      width: group.keeper.width,
      height: group.keeper.height,
      fileSize: group.keeper.file_size,
      ssim: 1.0,
      checked: false,
      isKeeper: true,
      groupIndex: gi,
    });

    // Duplicate rows
    group.duplicates.forEach((dup, di) => {
      const scoreEntry = group.scores.find(([p]) => p === dup.path);
      const ssim = scoreEntry ? scoreEntry[1] : 0;
      rows.push({
        groupNum,
        groupStart: false,
        action: "DELETE",
        path: dup.path,
        width: dup.width,
        height: dup.height,
        fileSize: dup.file_size,
        ssim,
        checked: true,  // duplicates checked by default
        isKeeper: false,
        groupIndex: gi,
      });
    });
  });
}

// ── Render table ────────────────────────────────────────────────────────

function renderTable() {
  const filtered = getFilteredRows();
  tbody.textContent = "";
  emptyState.style.display = filtered.length ? "none" : "";

  filtered.forEach((row) => {
    const tr = document.createElement("tr");
    tr.className = row.isKeeper ? "row-keeper" : "row-duplicate";
    if (row.groupStart) tr.classList.add("group-start");
    if (rows.indexOf(row) === selectedRowIdx) tr.classList.add("selected");

    // Checkbox
    const tdCheck = document.createElement("td");
    const cb = document.createElement("input");
    cb.type = "checkbox";
    cb.checked = row.checked;
    cb.addEventListener("change", (e) => {
      e.stopPropagation();
      row.checked = cb.checked;
      updateStats();
    });
    tdCheck.appendChild(cb);
    tr.appendChild(tdCheck);

    // Group
    const tdGroup = document.createElement("td");
    tdGroup.textContent = row.groupNum;
    tr.appendChild(tdGroup);

    // Action (clickable to toggle)
    const tdAction = document.createElement("td");
    tdAction.className = row.action === "KEEP" ? "action-keep" : "action-delete";
    tdAction.textContent = row.action;
    tdAction.style.cursor = "pointer";
    tdAction.addEventListener("click", (e) => {
      e.stopPropagation();
      row.action = row.action === "KEEP" ? "DELETE" : "KEEP";
      row.checked = row.action === "DELETE";
      tdAction.className = row.action === "KEEP" ? "action-keep" : "action-delete";
      tdAction.textContent = row.action;
      cb.checked = row.checked;
      updateStats();
    });
    tr.appendChild(tdAction);

    // Resolution
    const tdRes = document.createElement("td");
    tdRes.textContent = `${row.width}\u00D7${row.height}`;
    tr.appendChild(tdRes);

    // File size
    const tdSize = document.createElement("td");
    tdSize.textContent = formatSize(row.fileSize);
    tr.appendChild(tdSize);

    // SSIM
    const tdSsim = document.createElement("td");
    tdSsim.textContent = row.ssim >= 0.001 ? row.ssim.toFixed(4) : "\u2014";
    tr.appendChild(tdSsim);

    // Path
    const tdPath = document.createElement("td");
    tdPath.textContent = row.path;
    tdPath.title = row.path;
    tr.appendChild(tdPath);

    // Row click -> preview (Ctrl+click -> compare selection)
    tr.addEventListener("click", (e) => {
      if (e.ctrlKey) {
        // Add to compare selection
        if (compareSelection.length < 2 && !compareSelection.find(r => r.path === row.path)) {
          compareSelection.push(row);
          tr.classList.add("compare-selected");
        }
        if (compareSelection.length === 2) {
          btnCompare.disabled = false;
        }
        return;
      }
      // Clear compare selection on normal click
      compareSelection = [];
      btnCompare.disabled = true;
      document.querySelectorAll("tbody tr.compare-selected").forEach(el => el.classList.remove("compare-selected"));

      selectedRowIdx = rows.indexOf(row);
      document.querySelectorAll("tbody tr.selected").forEach(el => el.classList.remove("selected"));
      tr.classList.add("selected");
      showPreview(row.path, row);
    });

    tbody.appendChild(tr);
  });

  // Update header sort arrows
  document.querySelectorAll("thead th.sortable").forEach((th) => {
    const col = th.dataset.col;
    th.classList.toggle("sort-active", col === sortCol);
    let arrow = th.querySelector(".sort-arrow");
    if (!arrow) {
      arrow = document.createElement("span");
      arrow.className = "sort-arrow";
      th.appendChild(arrow);
    }
    arrow.textContent = col === sortCol ? (sortAsc ? " \u25B2" : " \u25BC") : "";
  });
}

// ── Sorting ─────────────────────────────────────────────────────────────

document.querySelectorAll("thead th.sortable").forEach((th) => {
  th.addEventListener("click", () => {
    const col = th.dataset.col;
    if (sortCol === col) {
      sortAsc = !sortAsc;
    } else {
      sortCol = col;
      sortAsc = true;
    }
    sortRows();
    renderTable();
  });
});

function sortRows() {
  rows.sort((a, b) => {
    let va, vb;
    switch (sortCol) {
      case "group": va = a.groupNum; vb = b.groupNum; break;
      case "action": va = a.action; vb = b.action; break;
      case "resolution": va = a.width * a.height; vb = b.width * b.height; break;
      case "size": va = a.fileSize; vb = b.fileSize; break;
      case "ssim": va = a.ssim; vb = b.ssim; break;
      case "path": va = a.path.toLowerCase(); vb = b.path.toLowerCase(); break;
      default: va = a.groupNum; vb = b.groupNum;
    }
    if (va < vb) return sortAsc ? -1 : 1;
    if (va > vb) return sortAsc ? 1 : -1;

    // Secondary sort: within same group, keeper first
    if (a.groupNum === b.groupNum) {
      return a.isKeeper ? -1 : 1;
    }
    return 0;
  });
  // Mark group starts
  let lastGroup = -1;
  rows.forEach((r) => {
    r.groupStart = r.groupNum !== lastGroup;
    lastGroup = r.groupNum;
  });
}

// ── Filtering ───────────────────────────────────────────────────────────

document.querySelectorAll(".filter-btn").forEach((btn) => {
  btn.addEventListener("click", () => {
    document.querySelectorAll(".filter-btn").forEach(b => b.classList.remove("active"));
    btn.classList.add("active");
    activeFilter = btn.dataset.filter;
    renderTable();
  });
});

function getFilteredRows() {
  if (activeFilter === "all") return rows;
  return rows.filter((r) =>
    activeFilter === "keep" ? r.action === "KEEP" : r.action === "DELETE"
  );
}

// ── Select all ──────────────────────────────────────────────────────────

selectAll.addEventListener("change", () => {
  const checked = selectAll.checked;
  getFilteredRows().forEach((r) => { r.checked = checked; });
  renderTable();
  updateStats();
});

// ── Preview ─────────────────────────────────────────────────────────────

async function showPreview(path, row) {
  zoomLevel = ZOOM_FIT;
  previewContent.classList.remove("zoomed");
  previewContent.textContent = "";
  const loadingMsg = document.createElement("p");
  loadingMsg.className = "subtle";
  loadingMsg.textContent = "Loading...";
  previewContent.appendChild(loadingMsg);
  previewInfo.textContent = "";

  try {
    const dataUrl = await invoke("get_image_base64", { path });
    const img = document.createElement("img");
    img.src = dataUrl;
    img.alt = path.split("\\").pop() || path.split("/").pop();
    previewContent.textContent = "";
    previewContent.appendChild(img);
    applyZoom();

    const filename = path.split("\\").pop() || path.split("/").pop();
    previewInfo.textContent = `${row.width}\u00D7${row.height} \u00B7 ${formatSize(row.fileSize)} \u00B7 ${filename}`;
    document.getElementById("zoom-level").textContent = "Fit";
  } catch (e) {
    previewContent.textContent = "";
    const errMsg = document.createElement("p");
    errMsg.className = "subtle";
    errMsg.textContent = "Cannot load preview: " + e;
    previewContent.appendChild(errMsg);
  }
}

// ── Resizer (drag to resize preview panel) ──────────────────────────────

const resizer = document.getElementById("resizer");
const previewPanel = document.getElementById("preview-panel");

let isResizing = false;

resizer.addEventListener("mousedown", (e) => {
  isResizing = true;
  resizer.classList.add("active");
  document.body.style.cursor = "col-resize";
  e.preventDefault();
});

document.addEventListener("mousemove", (e) => {
  if (!isResizing) return;
  const mainRect = document.getElementById("main").getBoundingClientRect();
  const newWidth = mainRect.right - e.clientX;
  if (newWidth >= 200 && newWidth <= 600) {
    previewPanel.style.width = newWidth + "px";
  }
});

document.addEventListener("mouseup", () => {
  if (isResizing) {
    isResizing = false;
    resizer.classList.remove("active");
    document.body.style.cursor = "";
  }
});

// ── Delete all duplicates ───────────────────────────────────────────────

btnDeleteAll.addEventListener("click", async () => {
  const toDelete = rows.filter((r) => r.checked && r.action === "DELETE");
  if (toDelete.length === 0) {
    statusText.textContent = "No files selected for deletion.";
    return;
  }

  const totalSize = toDelete.reduce((s, r) => s + r.fileSize, 0);
  const ok = await confirm(
    `Send ${toDelete.length} duplicate files (${formatSize(totalSize)}) to trash?`,
    { title: "Confirm Deletion", kind: "warning" }
  );

  if (!ok) return;

  statusText.textContent = "Deleting...";
  const paths = toDelete.map((r) => r.path);

  try {
    const errors = await invoke("send_to_trash", { paths });
    if (errors.length > 0) {
      statusText.textContent = `Deleted with ${errors.length} errors. Check console.`;
      console.error("Delete errors:", errors);
    } else {
      statusText.textContent = `Deleted ${toDelete.length} files, recovered ${formatSize(totalSize)}.`;
    }

    // Save to undo stack (deep copy the deleted rows)
    const deletedRows = toDelete.map(r => ({ ...r }));
    undoStack.push({ deletedRows, paths });
    showUndoToast(`Deleted ${toDelete.length} files (${formatSize(totalSize)})`);

    // Remove deleted rows
    const deletedSet = new Set(paths);
    rows = rows.filter((r) => !deletedSet.has(r.path));
    // Remove groups that now have < 2 members
    const groupCounts = {};
    rows.forEach((r) => {
      groupCounts[r.groupNum] = (groupCounts[r.groupNum] || 0) + 1;
    });
    rows = rows.filter((r) => groupCounts[r.groupNum] >= 2);
    // Re-number groups
    renumberGroups();
    renderTable();
    updateStats();

    if (rows.length === 0) {
      btnDeleteAll.disabled = true;
      btnExport.disabled = true;
      emptyState.style.display = "";
      emptyState.textContent = "All duplicates have been deleted!";
    }
  } catch (e) {
    statusText.textContent = `Error: ${e}`;
  }
});

function renumberGroups() {
  const seen = new Map();
  let num = 0;
  rows.forEach((r) => {
    if (!seen.has(r.groupIndex)) {
      num++;
      seen.set(r.groupIndex, num);
    }
    r.groupNum = seen.get(r.groupIndex);
  });
}

// ── Stats ───────────────────────────────────────────────────────────────

function updateStats() {
  const dupes = rows.filter((r) => r.action === "DELETE");
  const checked = rows.filter((r) => r.checked && r.action === "DELETE");
  const recoverable = checked.reduce((s, r) => s + r.fileSize, 0);
  statusStats.textContent = `${groups.length} groups \u00B7 ${dupes.length} duplicates \u00B7 ${formatSize(recoverable)} recoverable (${checked.length} selected)`;
}

// ── Zoom ────────────────────────────────────────────────────────────────

const ZOOM_STEPS = [0.25, 0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0];
const ZOOM_FIT = -1;
let zoomLevel = ZOOM_FIT; // -1 = fit

function applyZoom() {
  const img = previewContent.querySelector("img");
  if (!img) return;

  const zoomLevelLabel = document.getElementById("zoom-level");

  if (zoomLevel === ZOOM_FIT) {
    img.style.maxWidth = "100%";
    img.style.maxHeight = "100%";
    img.style.width = "";
    img.style.height = "";
    previewContent.classList.remove("zoomed");
    zoomLevelLabel.textContent = "Fit";
  } else {
    const pct = Math.round(zoomLevel * 100);
    img.style.maxWidth = "none";
    img.style.maxHeight = "none";
    img.style.width = (zoomLevel * 100) + "%";
    img.style.height = "auto";
    previewContent.classList.add("zoomed");
    zoomLevelLabel.textContent = pct + "%";
  }
}

function zoomIn() {
  if (zoomLevel === ZOOM_FIT) {
    zoomLevel = 1.0;
  } else {
    const next = ZOOM_STEPS.find(z => z > zoomLevel);
    if (next) zoomLevel = next;
  }
  applyZoom();
}

function zoomOut() {
  if (zoomLevel === ZOOM_FIT) {
    zoomLevel = 0.75;
  } else {
    const prev = [...ZOOM_STEPS].reverse().find(z => z < zoomLevel);
    if (prev) zoomLevel = prev;
  }
  applyZoom();
}

function zoomFit() {
  zoomLevel = ZOOM_FIT;
  applyZoom();
}

// Sidebar zoom controls
document.getElementById("zoom-in").addEventListener("click", zoomIn);
document.getElementById("zoom-out").addEventListener("click", zoomOut);
document.getElementById("zoom-fit").addEventListener("click", zoomFit);


// Block browser-level Ctrl+Wheel zoom globally (WebView2 page zoom)
document.addEventListener("wheel", (e) => {
  if (e.ctrlKey) e.preventDefault();
}, { passive: false });

// Ctrl+Wheel zoom on preview image
previewContent.addEventListener("wheel", (e) => {
  if (!e.ctrlKey) return;
  if (e.deltaY < 0) zoomIn();
  else zoomOut();
});

// ── Side-by-side comparison ─────────────────────────────────────────────

let compareZoom = ZOOM_FIT;

btnCompare.addEventListener("click", () => {
  if (compareSelection.length === 2) openCompare(compareSelection[0], compareSelection[1]);
});

function applyCompareZoom() {
  const imgA = document.getElementById("compare-img-a");
  const imgB = document.getElementById("compare-img-b");
  const label = document.getElementById("compare-zoom-level");
  const wrapA = imgA.parentElement;
  const wrapB = imgB.parentElement;

  if (compareZoom === ZOOM_FIT) {
    imgA.style.maxWidth = "100%"; imgA.style.maxHeight = "100%";
    imgA.style.width = ""; imgA.style.height = "";
    imgB.style.maxWidth = "100%"; imgB.style.maxHeight = "100%";
    imgB.style.width = ""; imgB.style.height = "";
    wrapA.style.alignItems = "center"; wrapA.style.justifyContent = "center";
    wrapB.style.alignItems = "center"; wrapB.style.justifyContent = "center";
    label.textContent = "Fit";
  } else {
    const pct = (compareZoom * 100) + "%";
    imgA.style.maxWidth = "none"; imgA.style.maxHeight = "none";
    imgA.style.width = pct; imgA.style.height = "auto";
    imgB.style.maxWidth = "none"; imgB.style.maxHeight = "none";
    imgB.style.width = pct; imgB.style.height = "auto";
    wrapA.style.alignItems = "flex-start"; wrapA.style.justifyContent = "flex-start";
    wrapB.style.alignItems = "flex-start"; wrapB.style.justifyContent = "flex-start";
    label.textContent = Math.round(compareZoom * 100) + "%";
  }
}

function compareZoomIn() {
  if (compareZoom === ZOOM_FIT) compareZoom = 1.0;
  else { const next = ZOOM_STEPS.find(z => z > compareZoom); if (next) compareZoom = next; }
  applyCompareZoom();
}

function compareZoomOut() {
  if (compareZoom === ZOOM_FIT) compareZoom = 0.75;
  else { const prev = [...ZOOM_STEPS].reverse().find(z => z < compareZoom); if (prev) compareZoom = prev; }
  applyCompareZoom();
}

document.getElementById("compare-zoom-in").addEventListener("click", compareZoomIn);
document.getElementById("compare-zoom-out").addEventListener("click", compareZoomOut);
document.getElementById("compare-zoom-fit").addEventListener("click", () => {
  compareZoom = ZOOM_FIT;
  applyCompareZoom();
});

// Ctrl+Wheel zoom on compare panes
document.getElementById("compare-body").addEventListener("wheel", (e) => {
  if (!e.ctrlKey) return;
  e.preventDefault();
  if (e.deltaY < 0) compareZoomIn();
  else compareZoomOut();
}, { passive: false });

async function openCompare(rowA, rowB) {
  const overlay = document.getElementById("compare-overlay");
  const imgA = document.getElementById("compare-img-a");
  const imgB = document.getElementById("compare-img-b");
  const infoA = document.getElementById("compare-info-a");
  const infoB = document.getElementById("compare-info-b");

  compareZoom = ZOOM_FIT;
  overlay.classList.remove("hidden");

  infoA.textContent = "Loading...";
  infoB.textContent = "Loading...";
  imgA.src = "";
  imgB.src = "";

  try {
    const [dataA, dataB] = await Promise.all([
      invoke("get_image_base64", { path: rowA.path }),
      invoke("get_image_base64", { path: rowB.path }),
    ]);
    imgA.src = dataA;
    imgB.src = dataB;
    applyCompareZoom();

    const nameA = rowA.path.split("\\").pop() || rowA.path.split("/").pop();
    const nameB = rowB.path.split("\\").pop() || rowB.path.split("/").pop();
    infoA.textContent = `${nameA}\n${rowA.width}×${rowA.height} · ${formatSize(rowA.fileSize)} · ${rowA.action}`;
    infoB.textContent = `${nameB}\n${rowB.width}×${rowB.height} · ${formatSize(rowB.fileSize)} · ${rowB.action}`;
  } catch (e) {
    infoA.textContent = "Error loading image";
    infoB.textContent = "Error loading image";
  }
}

document.getElementById("compare-close").addEventListener("click", () => {
  document.getElementById("compare-overlay").classList.add("hidden");
});

document.getElementById("compare-overlay").addEventListener("click", (e) => {
  if (e.target.id === "compare-overlay") {
    document.getElementById("compare-overlay").classList.add("hidden");
  }
});

// ── Undo delete ────────────────────────────────────────────────────────

let undoTimer = null;

function showUndoToast(message) {
  const toast = document.getElementById("undo-toast");
  document.getElementById("undo-message").textContent = message;
  toast.classList.remove("hidden");
  clearTimeout(undoTimer);
  undoTimer = setTimeout(() => toast.classList.add("hidden"), 8000);
}

document.getElementById("undo-btn").addEventListener("click", () => {
  if (undoStack.length === 0) return;
  const last = undoStack.pop();

  // Restore rows back into the table
  last.deletedRows.forEach(r => rows.push(r));
  // Re-sort and re-number
  sortRows();
  renumberGroups();
  renderTable();
  updateStats();

  btnDeleteAll.disabled = false;
  btnExport.disabled = false;
  emptyState.style.display = rows.length ? "none" : "";

  document.getElementById("undo-toast").classList.add("hidden");
  statusText.textContent = `Restored ${last.deletedRows.length} files to the list. (Files remain in trash on disk.)`;
});

// ── Drag and drop folder ───────────────────────────────────────────────

const dropOverlay = document.getElementById("drop-overlay");

listen("tauri://drag-enter", () => {
  dropOverlay.classList.remove("hidden");
});

listen("tauri://drag-leave", () => {
  dropOverlay.classList.add("hidden");
});

listen("tauri://drag-drop", (event) => {
  dropOverlay.classList.add("hidden");
  const paths = event.payload.paths;
  if (paths && paths.length > 0) {
    // Use the first dropped path
    folderPath.value = paths[0];
    btnScan.disabled = false;
    statusText.textContent = `Folder set: ${paths[0]}`;
  }
});

// ── Export CSV ──────────────────────────────────────────────────────────

btnExport.addEventListener("click", async () => {
  const path = await saveDialog({
    defaultPath: "image-dedup-report.csv",
    filters: [{ name: "CSV", extensions: ["csv"] }],
  });
  if (!path) return;

  const header = "Group,Action,Resolution,File Size,File Size (bytes),SSIM,Path";
  const csvRows = rows.map(r =>
    `${r.groupNum},${r.action},"${r.width}×${r.height}",${formatSize(r.fileSize)},${r.fileSize},${r.ssim.toFixed(4)},"${r.path.replace(/"/g, '""')}"`
  );
  const csv = [header, ...csvRows].join("\n");

  try {
    await invoke("export_csv", { path, csvContent: csv });
    statusText.textContent = `Exported ${rows.length} rows to ${path}`;
  } catch (e) {
    statusText.textContent = `Export error: ${e}`;
  }
});

// ── Helpers ─────────────────────────────────────────────────────────────

function formatSize(bytes) {
  if (bytes < 1024) return bytes + " B";
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + " KB";
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + " MB";
  return (bytes / (1024 * 1024 * 1024)).toFixed(2) + " GB";
}
