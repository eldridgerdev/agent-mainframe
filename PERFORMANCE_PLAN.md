# AMF Performance Analysis Plan

## Performance Issues Identified

### 1. ~~**View Mode - Frequent tmux capture_pane_ansi calls (50ms loop)**~~
- **Location**: `src/main.rs:219-235` - In viewing mode, captures full pane ANSI content every 50ms
- **Impact**: Subprocess overhead, large string allocation/parsing on every loop iteration
- **Severity**: HIGH - affects responsiveness especially with many active sessions
- **Status**: NOT STARTED

### 2. ~~**Thinking Status Sync - Scans ALL features every 500ms**~~ ✅ FIXED
- **Location**: `src/app/sync.rs:52-108` - `sync_thinking_status()`
- **Impact**: When running 10+ features, this spawns 10+ `capture_pane` subprocesses every 500ms
- **Severity**: HIGH - exponential cost with number of active sessions
- **Status**: FIXED - Removed polling for Claude, now relies solely on hooks (sentinel file at `/tmp/amf-thinking/{session}`)
- **Fix Details**:
  - Removed timer-based polling that was calling `capture_pane` for every Claude session
  - Removed `last_timer_values` HashMap that was tracking timer changes
  - Now Claude thinking detection uses only sentinel file created/removed by PreToolUse/Stop hooks
  - Opencode still uses polling (to be addressed later)
  - Added modification time checking (2 second threshold) to handle stale files after crashes
  - Added thinking touch to `UserPromptSubmit` hook to capture initial thinking phase
  - Added cleanup of stale files (>10 seconds) on app startup

### 3. **Notification Scan - File I/O every 500ms**
- **Location**: `src/app/notifications.rs:8-230` - `scan_notifications()`
- **Impact**: Scans all project notification directories, reads JSON files via filesystem I/O every 500ms
- **Severity**: MEDIUM - filesystem operations can be slow with many projects
- **Status**: NOT STARTED

### 4. **Redraw on every loop iteration**
- **Location**: `src/main.rs:281` - `terminal.draw()` called unconditionally
- **Impact**: Full UI render even when nothing changed, especially wasteful when pane content is static
- **Severity**: MEDIUM - GPU/CPU overhead for rendering
- **Status**: NOT STARTED

### 5. **Sync operations block the event loop**
- **Location**: `src/main.rs:291-310` - Status sync and session sync operations
- **Impact**: These spawn subprocesses and can take 10-50ms, blocking key event processing
- **Severity**: MEDIUM - causes keypress delays/lag
- **Status**: NOT STARTED

### 6. **Multiple cursor_position queries in view mode**
- **Location**: `src/main.rs:232-234` - Called every loop in viewing mode
- **Impact**: Additional tmux subprocess every 50ms even when cursor hasn't moved
- **Severity**: LOW-MEDIUM
- **Status**: NOT STARTED

### 7. **Notification path matching is O(n×m)**
- **Location**: `src/app/notifications.rs:124-133, 174-194` - Nested loops to match cwd paths
- **Impact**: With many projects/features, this becomes expensive
- **Severity**: LOW - but scales poorly
- **Status**: NOT STARTED

### 8. **No caching of pane content changes**
- **Impact**: Rerenders even when tmux output is identical
- **Severity**: LOW - wastes CPU on redundant redraws
- **Status**: NOT STARTED

## Recommended Fixes (Priority Order)

1. ~~**Batched thinking status sync**~~ ✅ COMPLETED - Removed Claude polling, use hooks with mtime checking
2. **Pane content change detection** - Hash content, skip capture if unchanged
3. **Adaptive notification scan** - Scan every 2s when idle, 500ms when inputs pending
4. **Move sync to background thread** - Don't block event loop
5. **Cursor position caching** - Only query when actually needed
6. **Redraw optimization** - Only draw when state changes

## Implementation Notes

### Issue 1: View Mode Frequent Captures
- Add content hashing to detect actual changes
- Only update `pane_content` when hash differs
- Cache hash in `App` struct

### Issue 2: Thinking Status Sync ✅ FIXED
- **Phase A (Modification Time Check)**:
  - Changed `is_claude_thinking()` to check file modification time instead of existence
  - 2 second threshold: file modified within 2 seconds = thinking
  - Handles crash scenarios where Stop hook doesn't run (old files become idle)
- **Phase B (Start Hook)**:
  - Added `Start` hook in `ensure_notification_hooks()` to create sentinel file immediately
  - Captures initial thinking phase before PreToolUse fires
  - Combined with mtime checking for robust detection
- **Stale File Cleanup**:
  - Added `cleanup_stale_thinking_files()` function
  - Removes files older than 10 seconds from `/tmp/amf-thinking/`
  - Called on app startup in `main()`
- Opencode still uses polling (TODO: implement hook-based detection)

### Issue 3: Notification Scan
- Check `app.pending_inputs.is_empty()` to determine scan frequency
- Use 2 second interval when idle, 500ms when pending inputs exist

### Issue 4: Redraw Optimization
- Add `needs_redraw` flag to `App`
- Set flag when state changes
- Only call `terminal.draw()` when flag is true

### Issue 5: Background Sync
- Use channels to communicate with background thread
- Thread runs sync operations, sends results back
- Main thread processes results non-blockingly

### Issue 6: Cursor Caching
- Store last cursor position in `App`
- Only call `cursor_position()` if content changed
- Or move to periodic update (every 200ms instead of 50ms)

### Issue 7: Path Matching Optimization
- Build a HashMap of workdir paths on startup
- O(1) lookup instead of O(n) linear scan
- Rebuild only when projects change

### Issue 8: Content Change Detection
- Covered in Issue 1
- Also applies to normal mode redraws (though less critical)

