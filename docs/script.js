const shortcuts = [
  ["General", ":", "Open the command palette and fuzzy-search every command"],
  ["General", "?", "Open the live help reference"],
  ["General", "r", "Refresh dashboard data from GitHub"],
  ["General", "q / Ctrl+C", "Save UI state and quit"],
  ["General", "1 / 2 / 3 / 4", "Focus ghr, Sections, List, or Details"],
  ["General", "Tab / Shift+Tab", "Switch list/details focus, or move within focused ghr/Sections tabs"],
  ["General", "h / l", "Move within focused ghr or Sections tabs"],
  ["General", "S", "Search pull requests and issues in the current repo"],
  ["General", "f", "Filter the current PR or issue section"],
  ["General", "o", "Open the selected item or PR changes page"],
  ["General", "m", "Toggle between TUI mouse controls and terminal text selection"],
  ["General", "i", "Ignore the selected pull request or issue"],
  ["List", "j / k / Up / Down", "Move list selection"],
  ["List", "[ / ]", "Load previous or next GitHub result page"],
  ["List", "PgUp / PgDown or u / d", "Move list selection by a visible page"],
  ["List", "g / G", "Jump to first or last item"],
  ["List", "/", "Fuzzy filter the loaded list"],
  ["List", "Tab / Shift+Tab", "Focus the Details pane"],
  ["List", "Enter", "Focus the Details pane"],
  ["List", "v", "Open pull request diff mode"],
  ["List", "T", "Edit selected issue or pull request title/body"],
  ["Issue/PR", "a", "Add a normal issue or PR comment"],
  ["Issue/PR", "L", "Add a label"],
  ["Issue/PR", "N", "Create an issue, or create a PR from local_dir in PR lists"],
  ["Issue/PR", "+", "Add a reaction to the item or focused comment"],
  ["Issue/PR", "@ / -", "Assign or unassign assignees"],
  ["Issue/PR", "t", "Change or clear milestone"],
  ["Pull Request", "M", "Open merge confirmation"],
  ["Pull Request", "C", "Close or reopen"],
  ["Pull Request", "X", "Checkout PR locally with gh pr checkout"],
  ["Pull Request", "F", "Rerun failed checks"],
  ["Pull Request", "U", "Update PR branch"],
  ["Pull Request", "D", "Toggle draft or ready for review"],
  ["Pull Request", "E / O", "Enable or disable auto-merge"],
  ["Pull Request", "P / Y", "Request or remove requested reviewers"],
  ["Review", "s", "Submit a PR review summary as a comment"],
  ["Review", "A", "Approve through the review summary flow"],
  ["Review", "Ctrl+D", "Discard a pending PR review"],
  ["Merge Dialog", "m / s / r", "Choose merge, squash, or rebase"],
  ["Merge Dialog", "Tab", "Cycle merge method"],
  ["Dialog", "y / Enter", "Confirm the current action"],
  ["Editor", "Ctrl+Enter", "Send, update, create, or submit from editor dialogs"],
  ["Editor", "Esc", "Cancel or close the current dialog"],
  ["Details", "j / k / Up / Down", "Scroll conversation details or move diff cursor"],
  ["Details", "PgUp / PgDown or u / d", "Scroll details by page"],
  ["Details", "Tab / Shift+Tab", "Focus the list pane"],
  ["Details", "g / G", "Scroll to top or bottom"],
  ["Details", "/", "Search loaded comments by keyword"],
  ["Details", "n / p", "Focus next or previous comment"],
  ["Details", "Enter", "Expand or collapse a long focused comment"],
  ["Details", "R", "Reply to the focused comment"],
  ["Details", "e", "Edit the focused comment when it is yours"],
  ["Details", "c / a", "Add a comment in conversation mode"],
  ["Diff", "q / Esc", "Return to the state before opening diff mode"],
  ["Diff", "Tab / Shift+Tab", "Switch focus between changed files and the file diff"],
  ["Diff Files", "j / k / Up / Down", "Choose a changed file"],
  ["Diff Files", "PgUp / PgDown or u / d", "Move by a visible file page"],
  ["Diff Files", "[ / ]", "Previous or next changed file"],
  ["Diff Files", "Enter / 4", "Focus the file diff"],
  ["Diff Details", "h / l", "Page down or page up through the current file diff"],
  ["Diff Details", "j / k", "Move selected diff line"],
  ["Diff Details", "m", "Begin a review range"],
  ["Diff Details", "e", "End the review range"],
  ["Diff Details", "c", "Add an inline review comment"],
  ["Diff Details", "single click", "Select one line, or end a pending range"],
  ["Diff Details", "double click", "Begin a review range"],
  ["Project", "Palette", "Project Switch: filter configured repositories and switch tabs"],
  ["Project", "Palette", "Project Add: add a repo project to config.toml"],
  ["Project", "Palette", "Project Remove: remove a repo project from config.toml"],
  ["Cache", "Palette", "Clear Cache: choose which local cache layer to clear"],
  ["Inbox", "Palette", "Mark All Read: mark every GitHub inbox notification as read"],
  ["Inbox", "Palette", "Mute Thread: ignore future notifications for the selected inbox thread"],
  ["Inbox", "Palette", "Subscribe Thread: subscribe to the selected inbox thread"],
  ["Inbox", "Palette", "Unsubscribe Thread: unsubscribe from the selected inbox thread"],
  ["Clipboard", "Palette", "Copy GitHub Link: copy selected comment link or issue/PR URL"],
  ["Clipboard", "Palette", "Copy Content: copy selected comment body or issue/PR description"],
  ["Info", "Palette", "Info: show version, paths, ghr process memory, UI state, and ignored count"],
  ["Mouse", "click tabs / sections", "Switch view or section"],
  ["Mouse", "click list row", "Select item or diff file"],
  ["Mouse", "click links/actions", "Open links or run visible actions"],
  ["Mouse", "wheel", "Scroll list, details, or dialog under the pointer"],
  ["Mouse", "drag split border", "Resize list/details ratio"],
];

const table = document.querySelector("#shortcut-table");
const input = document.querySelector("#shortcut-filter");

function renderShortcuts(query = "") {
  const normalized = query.trim().toLowerCase();
  const matches = shortcuts.filter(([scope, key, action]) => {
    const text = `${scope} ${key} ${action}`.toLowerCase();
    return !normalized || text.includes(normalized);
  });

  table.replaceChildren();

  if (matches.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty-state";
    empty.textContent = "No shortcuts match that filter.";
    table.append(empty);
    return;
  }

  for (const [scope, key, action] of matches) {
    const row = document.createElement("div");
    row.className = "shortcut-row";
    row.innerHTML = `
      <span class="shortcut-scope"></span>
      <span class="shortcut-key"></span>
      <span class="shortcut-action"></span>
    `;
    row.children[0].textContent = scope;
    row.children[1].textContent = key;
    row.children[2].textContent = action;
    table.append(row);
  }
}

input.addEventListener("input", (event) => renderShortcuts(event.target.value));
renderShortcuts();
