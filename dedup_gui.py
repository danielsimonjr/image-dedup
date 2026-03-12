"""
Image Dedup — Tkinter GUI for finding and removing duplicate images.

Uses pHash (perceptual hashing) for fast candidate detection,
then SSIM (Structural Similarity Index) for verification.
Keeps the highest-resolution version of each duplicate group.
"""

import os
import sys
import threading
import tkinter as tk
from tkinter import ttk, filedialog, messagebox
from pathlib import Path

from PIL import Image, ImageTk

# Try Rust core first, fall back to pure-Python engine
sys.path.insert(0, str(Path(__file__).parent))
try:
    import dedup_core as engine

    BACKEND = "Rust (dedup_core)"
except ImportError:
    import dedup_engine as engine

    BACKEND = "Python (dedup_engine)"

# Checkbox characters
CHECK_ON = "\u2611"  # ☑
CHECK_OFF = "\u2610"  # ☐


def _delete_files(paths: list[str], send_to_trash: bool = True) -> list[str]:
    """Delete files, using send2trash when available."""
    deleted = []
    for p in paths:
        try:
            if send_to_trash:
                try:
                    from send2trash import send2trash as trash

                    trash(p)
                except ImportError:
                    Path(p).unlink()
            else:
                Path(p).unlink()
            deleted.append(p)
        except OSError:
            pass
    return deleted


class DedupApp:
    def __init__(self, root: tk.Tk):
        self.root = root
        self.root.title(f"Image Dedup \u2014 pHash + SSIM [{BACKEND}]")
        self.root.geometry("1100x750")
        self.root.minsize(900, 600)

        # State
        self.groups = []
        self.images = []
        self.preview_refs = []  # prevent GC
        self.selected_for_deletion: set[str] = set()
        self._scan_thread: threading.Thread | None = None
        self._pulse_active = False

        self._build_ui()

    # ── UI Construction ──────────────────────────────────────────────

    def _build_ui(self):
        # Top controls frame
        ctrl = ttk.Frame(self.root, padding=8)
        ctrl.pack(fill=tk.X)

        ttk.Label(ctrl, text="Folder:").pack(side=tk.LEFT)
        self.folder_var = tk.StringVar()
        self.folder_entry = ttk.Entry(ctrl, textvariable=self.folder_var, width=50)
        self.folder_entry.pack(side=tk.LEFT, padx=(4, 4))
        ttk.Button(ctrl, text="Browse...", command=self._browse).pack(side=tk.LEFT)

        self.recursive_var = tk.BooleanVar(value=True)
        ttk.Checkbutton(ctrl, text="Recursive", variable=self.recursive_var).pack(
            side=tk.LEFT, padx=(12, 4)
        )

        self.scan_btn = ttk.Button(ctrl, text="Scan", command=self._start_scan)
        self.scan_btn.pack(side=tk.LEFT, padx=(12, 4))

        self.delete_all_btn = ttk.Button(
            ctrl, text="Delete All Duplicates", command=self._delete_all_dupes
        )
        self.delete_all_btn.pack(side=tk.LEFT, padx=(4, 4))

        # Threshold controls
        thresh_frame = ttk.Frame(self.root, padding=(8, 0, 8, 4))
        thresh_frame.pack(fill=tk.X)

        ttk.Label(thresh_frame, text="pHash threshold:").pack(side=tk.LEFT)
        self.phash_thresh_var = tk.IntVar(value=10)
        phash_spin = ttk.Spinbox(
            thresh_frame,
            from_=1,
            to=25,
            textvariable=self.phash_thresh_var,
            width=5,
        )
        phash_spin.pack(side=tk.LEFT, padx=(4, 16))

        ttk.Label(thresh_frame, text="SSIM threshold:").pack(side=tk.LEFT)
        self.ssim_thresh_var = tk.DoubleVar(value=0.90)
        ssim_spin = ttk.Spinbox(
            thresh_frame,
            from_=0.50,
            to=1.00,
            increment=0.01,
            textvariable=self.ssim_thresh_var,
            width=6,
            format="%.2f",
        )
        ssim_spin.pack(side=tk.LEFT, padx=(4, 16))

        ttk.Label(thresh_frame, text="Min size (px):").pack(side=tk.LEFT)
        self.min_size_var = tk.IntVar(value=50)
        min_size_spin = ttk.Spinbox(
            thresh_frame,
            from_=1,
            to=500,
            textvariable=self.min_size_var,
            width=5,
        )
        min_size_spin.pack(side=tk.LEFT, padx=(4, 16))

        self.status_var = tk.StringVar(value="Select a folder and click Scan.")
        ttk.Label(thresh_frame, textvariable=self.status_var).pack(
            side=tk.LEFT, padx=(16, 0)
        )

        # Progress bar — use determinate mode with manual pulsing for compatibility
        self.progress = ttk.Progressbar(self.root, mode="determinate", maximum=100)
        self.progress.pack(fill=tk.X, padx=8, pady=(0, 4))

        # Main paned window: left = tree, right = preview
        paned = ttk.PanedWindow(self.root, orient=tk.HORIZONTAL)
        paned.pack(fill=tk.BOTH, expand=True, padx=8, pady=4)

        # Left: Treeview with duplicate groups
        tree_frame = ttk.Frame(paned)
        paned.add(tree_frame, weight=3)

        columns = ("selected", "action", "resolution", "size", "ssim", "path")
        self.tree = ttk.Treeview(
            tree_frame, columns=columns, show="tree headings", selectmode="browse"
        )
        self.tree.heading("#0", text="Group")
        self.tree.heading("selected", text=CHECK_ON)
        self.tree.heading("action", text="Action")
        self.tree.heading("resolution", text="Resolution")
        self.tree.heading("size", text="File Size")
        self.tree.heading("ssim", text="SSIM")
        self.tree.heading("path", text="Path")

        self.tree.column("#0", width=80, minwidth=60)
        self.tree.column("selected", width=40, minwidth=35, anchor="center")
        self.tree.column("action", width=60, minwidth=50)
        self.tree.column("resolution", width=90, minwidth=70)
        self.tree.column("size", width=80, minwidth=60)
        self.tree.column("ssim", width=60, minwidth=50)
        self.tree.column("path", width=280, minwidth=100)

        tree_scroll = ttk.Scrollbar(
            tree_frame, orient=tk.VERTICAL, command=self.tree.yview
        )
        self.tree.configure(yscrollcommand=tree_scroll.set)
        self.tree.pack(side=tk.LEFT, fill=tk.BOTH, expand=True)
        tree_scroll.pack(side=tk.RIGHT, fill=tk.Y)

        self.tree.bind("<<TreeviewSelect>>", self._on_select)
        self.tree.bind("<Button-1>", self._on_click)

        # Right: Preview panel — use tk.Label (not ttk) for reliable image display
        preview_frame = ttk.LabelFrame(paned, text="Preview", padding=4)
        paned.add(preview_frame, weight=2)

        self.preview_label = tk.Label(
            preview_frame,
            text="Select an image to preview",
            bg="#f0f0f0",
            anchor="center",
        )
        self.preview_label.pack(fill=tk.BOTH, expand=True)

        self.info_label = ttk.Label(preview_frame, text="", wraplength=350)
        self.info_label.pack(fill=tk.X, pady=(4, 0))

        # Bottom action bar
        action_frame = ttk.Frame(self.root, padding=8)
        action_frame.pack(fill=tk.X)

        ttk.Label(action_frame, text="Filter:").pack(side=tk.LEFT)
        ttk.Button(
            action_frame, text="Show All", command=lambda: self._filter_tree("all")
        ).pack(side=tk.LEFT, padx=(4, 2))
        ttk.Button(
            action_frame,
            text="Show KEEP Only",
            command=lambda: self._filter_tree("keep"),
        ).pack(side=tk.LEFT, padx=2)
        ttk.Button(
            action_frame,
            text="Show DELETE Only",
            command=lambda: self._filter_tree("delete"),
        ).pack(side=tk.LEFT, padx=2)

        ttk.Separator(action_frame, orient=tk.VERTICAL).pack(
            side=tk.LEFT, fill=tk.Y, padx=(12, 12)
        )

        self.select_all_btn = ttk.Button(
            action_frame, text="Check All Duplicates", command=self._select_all_dupes
        )
        self.select_all_btn.pack(side=tk.LEFT)

        self.deselect_btn = ttk.Button(
            action_frame, text="Uncheck All", command=self._deselect_all
        )
        self.deselect_btn.pack(side=tk.LEFT, padx=(8, 0))

        self.delete_btn = ttk.Button(
            action_frame,
            text="Delete Checked (Trash)",
            command=self._delete_selected,
            style="Accent.TButton",
        )
        self.delete_btn.pack(side=tk.RIGHT)

        self.count_var = tk.StringVar(value="")
        ttk.Label(action_frame, textvariable=self.count_var).pack(
            side=tk.RIGHT, padx=(0, 16)
        )

        # Style
        style = ttk.Style()
        try:
            style.configure("Accent.TButton", foreground="red")
        except Exception:
            pass

    # ── Progress pulsing (works reliably on all Windows builds) ───────

    def _start_pulse(self):
        self._pulse_active = True
        self._pulse_value = 0
        self._pulse_direction = 3
        self.progress.configure(value=0, maximum=100)
        self._do_pulse()

    def _do_pulse(self):
        if not self._pulse_active:
            return
        self._pulse_value += self._pulse_direction
        if self._pulse_value >= 100 or self._pulse_value <= 0:
            self._pulse_direction = -self._pulse_direction
        self.progress.configure(value=self._pulse_value)
        self.root.after(30, self._do_pulse)

    def _stop_pulse(self):
        self._pulse_active = False
        self.progress.configure(value=0)

    # ── Actions ──────────────────────────────────────────────────────

    def _browse(self):
        folder = filedialog.askdirectory(title="Select Image Folder")
        if folder:
            self.folder_var.set(folder)

    def _start_scan(self):
        folder = self.folder_var.get().strip()
        if not folder or not Path(folder).is_dir():
            messagebox.showwarning("Invalid Folder", "Please select a valid folder.")
            return

        if self._scan_thread and self._scan_thread.is_alive():
            messagebox.showinfo("Busy", "A scan is already in progress.")
            return

        self.scan_btn.configure(state="disabled")
        self.tree.delete(*self.tree.get_children())
        self.groups = []
        self.selected_for_deletion.clear()
        self._update_count()

        self._scan_thread = threading.Thread(
            target=self._scan_worker, args=(folder,), daemon=True
        )
        self._scan_thread.start()

    def _scan_worker(self, folder: str):
        """Runs in background thread."""
        try:
            # Phase 1: Scan images
            self.root.after(0, lambda: self.status_var.set("Scanning images..."))
            self.root.after(0, self._start_pulse)

            if BACKEND.startswith("Rust"):
                min_sz = self.min_size_var.get()
                images = engine.scan_images(
                    folder,
                    recursive=self.recursive_var.get(),
                    min_width=min_sz,
                    min_height=min_sz,
                )
            else:

                def scan_progress(done, total, current):
                    pct = int(done / max(total, 1) * 100)
                    name = Path(current).name
                    self.root.after(
                        0, lambda: self.progress.configure(value=pct, maximum=100)
                    )
                    self.root.after(
                        0,
                        lambda: self.status_var.set(f"Scanning {done}/{total}: {name}"),
                    )

                images = engine.scan_images(
                    folder,
                    recursive=self.recursive_var.get(),
                    progress_callback=scan_progress,
                )

            self.images = images

            n = len(images)
            if n < 2:
                self.root.after(0, self._stop_pulse)
                self.root.after(
                    0,
                    lambda: self.status_var.set(
                        f"Found {n} image(s) \u2014 need at least 2 to compare."
                    ),
                )
                self.root.after(0, lambda: self.scan_btn.configure(state="normal"))
                return

            # Phase 2: Find duplicates
            self.root.after(
                0,
                lambda: self.status_var.set(
                    f"Comparing {n} images (pHash + SSIM via {BACKEND})..."
                ),
            )

            if BACKEND.startswith("Rust"):
                groups = engine.find_duplicates(
                    images,
                    phash_threshold=self.phash_thresh_var.get(),
                    ssim_threshold=self.ssim_thresh_var.get(),
                )
            else:

                def match_progress(done, total, info):
                    pct = int(done / max(total, 1) * 100)
                    self.root.after(
                        0, lambda: self.progress.configure(value=pct, maximum=100)
                    )
                    self.root.after(
                        0,
                        lambda: self.status_var.set(
                            f"Comparing {done}/{total}: {info}"
                        ),
                    )

                groups = engine.find_duplicates(
                    images,
                    phash_threshold=self.phash_thresh_var.get(),
                    ssim_threshold=self.ssim_thresh_var.get(),
                    progress_callback=match_progress,
                )

            self.root.after(0, self._stop_pulse)
            self.groups = groups

            # Update UI on main thread
            self.root.after(0, lambda: self._populate_tree(groups))

        except Exception as e:
            self.root.after(0, self._stop_pulse)
            self.root.after(0, lambda: messagebox.showerror("Error", str(e)))
        finally:
            self.root.after(0, lambda: self.scan_btn.configure(state="normal"))

    def _populate_tree(self, groups):
        self.tree.delete(*self.tree.get_children())
        self.selected_for_deletion.clear()

        total_dupes = sum(len(g.duplicates) for g in groups)
        total_savings = sum(sum(d.file_size for d in g.duplicates) for g in groups)
        savings_mb = total_savings / (1024 * 1024)

        self.status_var.set(
            f"Found {len(groups)} duplicate group(s), "
            f"{total_dupes} file(s) to remove, "
            f"~{savings_mb:.1f} MB recoverable."
        )
        self.progress.configure(value=100)

        for i, group in enumerate(groups):
            group_id = f"group_{i}"

            # Parent row = keeper
            keeper = group.keeper
            self.tree.insert(
                "",
                tk.END,
                iid=group_id,
                text=f"Group {i + 1}",
                values=(
                    "",
                    "KEEP",
                    keeper.resolution_label,
                    _fmt_size(keeper.file_size),
                    "\u2014",
                    str(keeper.path),
                ),
                tags=("keeper",),
            )

            # Child rows = duplicates (auto-checked for deletion)
            scores_dict = (
                group.scores if isinstance(group.scores, dict) else dict(group.scores)
            )
            for j, dupe in enumerate(group.duplicates):
                child_id = f"group_{i}_dupe_{j}"
                score = scores_dict.get(str(dupe.path), 0.0)
                self.tree.insert(
                    group_id,
                    tk.END,
                    iid=child_id,
                    text="",
                    values=(
                        CHECK_ON,
                        "DELETE",
                        dupe.resolution_label,
                        _fmt_size(dupe.file_size),
                        f"{score:.3f}",
                        str(dupe.path),
                    ),
                    tags=("duplicate",),
                )
                self.selected_for_deletion.add(str(dupe.path))

        # Style tags
        self.tree.tag_configure("keeper", foreground="green")
        self.tree.tag_configure("duplicate", foreground="#cc0000")
        self.tree.tag_configure("skipped", foreground="gray")

        # Expand all groups
        for child in self.tree.get_children():
            self.tree.item(child, open=True)

        self._update_count()

    # ── Click handling: toggle checkbox + show preview ────────────────

    def _on_click(self, event):
        """Handle click — toggle checkbox if clicking the checkbox column."""
        region = self.tree.identify_region(event.x, event.y)
        if region != "cell":
            return

        col = self.tree.identify_column(event.x)
        item = self.tree.identify_row(event.y)
        if not item:
            return

        # col "#1" = selected column (checkbox)
        if col == "#1":
            self._toggle_check(item)

    def _toggle_check(self, item):
        values = list(self.tree.item(item, "values"))
        if not values or values[1] == "KEEP":
            return  # can't toggle keeper

        path = values[5]
        if values[0] == CHECK_ON:
            # Uncheck
            values[0] = CHECK_OFF
            values[1] = "SKIP"
            self.tree.item(item, values=values, tags=("skipped",))
            self.selected_for_deletion.discard(path)
        else:
            # Check
            values[0] = CHECK_ON
            values[1] = "DELETE"
            self.tree.item(item, values=values, tags=("duplicate",))
            self.selected_for_deletion.add(path)

        self._update_count()

    def _on_select(self, event):
        sel = self.tree.selection()
        if not sel:
            return

        item = sel[0]
        values = self.tree.item(item, "values")
        if not values:
            return

        img_path = values[5]  # path column
        self._show_preview(img_path)

        info_parts = [
            f"Path: {img_path}",
            f"Resolution: {values[2]}",
            f"Size: {values[3]}",
        ]
        if values[4] != "\u2014":
            info_parts.append(f"SSIM score: {values[4]}")
        self.info_label.configure(text="\n".join(info_parts))

    def _show_preview(self, img_path: str):
        try:
            with Image.open(img_path) as img:
                img.load()
                # Get preview area size
                pw = max(self.preview_label.winfo_width(), 200)
                ph = max(self.preview_label.winfo_height(), 200)
                img.thumbnail((pw, ph), Image.LANCZOS)
                photo = ImageTk.PhotoImage(img)
                self.preview_refs = [photo]  # prevent GC
                self.preview_label.configure(image=photo, text="")
        except Exception as e:
            self.preview_refs = []
            self.preview_label.configure(image="", text=f"Cannot load:\n{e}")

    # ── Filter tree view ─────────────────────────────────────────────

    def _filter_tree(self, mode: str):
        """Show/hide rows based on filter mode: 'all', 'keep', 'delete'."""
        for group_id in self.tree.get_children():
            group_has_visible = False

            for child_id in self.tree.get_children(group_id):
                values = self.tree.item(child_id, "values")
                action = values[1] if values else ""

                if mode == "all":
                    # Treeview doesn't support hide/show natively,
                    # so we reattach detached items. Use detach approach.
                    pass  # handled below by repopulating
                elif mode == "delete" and action not in ("DELETE",):
                    continue
                elif mode == "keep":
                    continue
                group_has_visible = True

        # Simplest reliable approach: repopulate the tree with filter
        if not self.groups:
            return

        self.tree.delete(*self.tree.get_children())

        for i, group in enumerate(self.groups):
            group_id = f"group_{i}"
            keeper = group.keeper
            scores_dict = (
                group.scores if isinstance(group.scores, dict) else dict(group.scores)
            )

            if mode == "delete":
                # Only show groups that have checked duplicates
                checked_dupes = [
                    d
                    for d in group.duplicates
                    if str(d.path) in self.selected_for_deletion
                ]
                if not checked_dupes:
                    continue

            # Insert keeper row (always visible unless delete-only filter)
            if mode != "delete":
                self.tree.insert(
                    "",
                    tk.END,
                    iid=group_id,
                    text=f"Group {i + 1}",
                    values=(
                        "",
                        "KEEP",
                        keeper.resolution_label,
                        _fmt_size(keeper.file_size),
                        "\u2014",
                        str(keeper.path),
                    ),
                    tags=("keeper",),
                )
                parent = group_id
            else:
                # In delete-only mode, use flat list (no grouping)
                parent = ""

            for j, dupe in enumerate(group.duplicates):
                child_id = f"group_{i}_dupe_{j}"
                path = str(dupe.path)
                is_checked = path in self.selected_for_deletion
                score = scores_dict.get(path, 0.0)

                if mode == "keep":
                    continue  # skip duplicates in keep-only view
                if mode == "delete" and not is_checked:
                    continue

                self.tree.insert(
                    parent,
                    tk.END,
                    iid=child_id,
                    text="" if parent else f"Group {i + 1}",
                    values=(
                        CHECK_ON if is_checked else CHECK_OFF,
                        "DELETE" if is_checked else "SKIP",
                        dupe.resolution_label,
                        _fmt_size(dupe.file_size),
                        f"{score:.3f}",
                        path,
                    ),
                    tags=("duplicate" if is_checked else "skipped",),
                )

        # Re-apply tag styles and expand
        self.tree.tag_configure("keeper", foreground="green")
        self.tree.tag_configure("duplicate", foreground="#cc0000")
        self.tree.tag_configure("skipped", foreground="gray")
        for child in self.tree.get_children():
            self.tree.item(child, open=True)

    # ── Bulk actions ─────────────────────────────────────────────────

    def _select_all_dupes(self):
        self.selected_for_deletion.clear()
        for group_id in self.tree.get_children():
            for child_id in self.tree.get_children(group_id):
                values = list(self.tree.item(child_id, "values"))
                if values[1] == "KEEP":
                    continue
                values[0] = CHECK_ON
                values[1] = "DELETE"
                self.tree.item(child_id, values=values, tags=("duplicate",))
                self.selected_for_deletion.add(values[5])
        self._update_count()

    def _deselect_all(self):
        self.selected_for_deletion.clear()
        for group_id in self.tree.get_children():
            for child_id in self.tree.get_children(group_id):
                values = list(self.tree.item(child_id, "values"))
                if values[1] == "KEEP":
                    continue
                values[0] = CHECK_OFF
                values[1] = "SKIP"
                self.tree.item(child_id, values=values, tags=("skipped",))
        self._update_count()

    def _update_count(self):
        n = len(self.selected_for_deletion)
        if n == 0:
            self.count_var.set("")
        else:
            total = 0
            for g in self.groups:
                for d in g.duplicates:
                    if str(d.path) in self.selected_for_deletion:
                        total += d.file_size
            mb = total / (1024 * 1024)
            self.count_var.set(f"{n} file(s) checked ({mb:.1f} MB)")

    def _delete_selected(self):
        if not self.selected_for_deletion:
            messagebox.showinfo("Nothing Checked", "No files are checked for deletion.")
            return

        n = len(self.selected_for_deletion)
        if not messagebox.askyesno(
            "Confirm Deletion",
            f"Send {n} file(s) to the Recycle Bin?\n\n"
            "Files are recoverable from the Recycle Bin.",
        ):
            return

        paths_to_delete = list(self.selected_for_deletion)
        deleted = _delete_files(paths_to_delete, send_to_trash=True)

        messagebox.showinfo(
            "Done",
            f"Deleted {len(deleted)} file(s) to Recycle Bin.",
        )

        self.selected_for_deletion -= set(deleted)
        self._refresh_tree_after_delete(deleted)

    def _delete_all_dupes(self):
        """Quick delete all duplicates without needing to scan the tree."""
        if not self.groups:
            messagebox.showinfo("No Results", "Run a scan first.")
            return

        # Gather all duplicate paths
        all_dupe_paths = set()
        for g in self.groups:
            for d in g.duplicates:
                all_dupe_paths.add(str(d.path))

        if not all_dupe_paths:
            messagebox.showinfo("Nothing to Delete", "No duplicates found.")
            return

        total_bytes = sum(
            d.file_size
            for g in self.groups
            for d in g.duplicates
            if str(d.path) in all_dupe_paths
        )
        mb = total_bytes / (1024 * 1024)

        if not messagebox.askyesno(
            "Delete ALL Duplicates",
            f"Send ALL {len(all_dupe_paths)} duplicate file(s) to the Recycle Bin?\n"
            f"This will free ~{mb:.1f} MB.\n\n"
            "The highest-resolution version of each group will be kept.\n"
            "Files are recoverable from the Recycle Bin.",
        ):
            return

        deleted = _delete_files(list(all_dupe_paths), send_to_trash=True)

        messagebox.showinfo(
            "Done",
            f"Deleted {len(deleted)} file(s) to Recycle Bin.",
        )

        self.selected_for_deletion -= set(deleted)
        self._refresh_tree_after_delete(deleted)

    def _refresh_tree_after_delete(self, deleted_paths: list[str]):
        deleted_set = set(deleted_paths)
        for group_id in list(self.tree.get_children()):
            for child_id in list(self.tree.get_children(group_id)):
                values = self.tree.item(child_id, "values")
                if values and values[5] in deleted_set:
                    self.tree.delete(child_id)
            # Remove empty groups
            if not self.tree.get_children(group_id):
                self.tree.delete(group_id)
        self._update_count()


def _fmt_size(nbytes: int) -> str:
    if nbytes < 1024:
        return f"{nbytes} B"
    elif nbytes < 1024 * 1024:
        return f"{nbytes / 1024:.1f} KB"
    else:
        return f"{nbytes / (1024 * 1024):.1f} MB"


def main():
    root = tk.Tk()
    DedupApp(root)
    root.mainloop()


if __name__ == "__main__":
    main()
