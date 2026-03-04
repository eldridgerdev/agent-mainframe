# Claude Thinking Detection Analysis

## Current Implementation Problems

### Issue 1: Sentinel file persists after crashes
When Claude crashes or is killed unexpectedly, the `Stop` hook doesn't run, leaving `/tmp/amf-thinking/{session}` file forever. This causes AMF to show Claude as "thinking" indefinitely.

**Example:** `/tmp/amf-thinking/amf-broken-release` is 2.5 hours old but still present.

✅ **FIXED**: Added modification time checking - files older than 2 seconds are not considered thinking

### Issue 2: PreToolUse hook fires too late
The `PreToolUse` hook only fires when Claude starts using a tool (Edit, Write, etc.), NOT when Claude is initially thinking/planning. This means:
- Claude starts thinking → no sentinel file yet
- Claude decides to use a tool → PreToolUse fires → sentinel file created
- User sees "thinking" only AFTER tool use starts

✅ **FIXED**: Added thinking touch command to `UserPromptSubmit` hook, which fires when user submits a prompt - this catches initial thinking phase before tool use

### Issue 3: Gaps between hooks
During long tool operations, there may be gaps where no hooks fire, but we need to indicate Claude is still active.

✅ **MITIGATED**: With mtime checking, as long as ANY hook touches the file within 2 seconds, we show thinking

### Issue 4: Race conditions
The hook execution and our sync check can race:
- Hook creates file
- Our check sees it and shows "thinking"
- Hook removes it
- Next check shows "idle"
- But Claude might still be processing

✅ **MITIGATED**: With 2 second threshold, small timing gaps don't cause false negatives

## Proposed Solutions

### Solution A: File Modification Time ✅ IMPLEMENTED

Instead of checking file existence, check the file's modification time:

**Implementation:**
```rust
fn is_claude_thinking(tmux_session: &str) -> bool {
    let path_str = format!("/tmp/amf-thinking/{}", tmux_session);
    let path = std::path::Path::new(&path_str);
    if !path.exists() {
        return false;
    }

    match std::fs::metadata(path) {
        Ok(metadata) => {
            match metadata.modified() {
                Ok(modified) => {
                    match modified.elapsed() {
                        Ok(elapsed) => elapsed < std::time::Duration::from_secs(2),
                        Err(_) => false,
                    }
                }
                Err(_) => false,
            }
        }
        Err(_) => false,
    }
}
```

**Benefits:**
- ✅ Hooks already use `touch` which updates mtime
- ✅ Works even if file persists after crash (it becomes "old" and we don't show thinking)
- ✅ No changes to hooks needed for mtime checking
- ✅ More resilient to crashes

**Additional cleanup:**
```rust
pub fn cleanup_stale_thinking_files() {
    let Ok(entries) = std::fs::read_dir("/tmp/amf-thinking") else {
        return;
    };

    for entry in entries.flatten() {
        if let Ok(metadata) = entry.metadata() {
            if let Ok(modified) = metadata.modified() {
                if let Ok(elapsed) = modified.elapsed() {
                    if elapsed > std::time::Duration::from_secs(10) {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
        }
    }
}
```

Called on app startup in `main()`.

---

### Solution B: Use UserPromptSubmit Hook ✅ IMPLEMENTED

Add thinking touch command to `UserPromptSubmit` hook, which fires when user submits a prompt:

**Implementation in `src/app/setup.rs`:**
```rust
// UserPromptSubmit: touch thinking sentinel + save latest prompt text.
hooks_obj.insert("UserPromptSubmit".to_string(), serde_json::json!([{
    "matcher": "",
    "hooks": [
        { "type": "command", "command": thinking_touch_cmd },
        { "type": "command", "command": save_prompt_cmd }
    ]
}]));
```

**Benefits:**
- ✅ Captures initial thinking phase immediately when user submits prompt
- ✅ No new hooks needed - reuses existing UserPromptSubmit
- ✅ Fires before PreToolUse, so we catch planning phase
- ✅ Still needs cleanup for stale files (solved by Solution A)
- ✅ Combined with Solution A for robust detection

**Drawbacks:**
- Still relies on Stop hook to remove file (mitigated by mtime checking)
- Doesn't solve crash persistence issue (solved by mtime checking)

---

### Solution C: Multi-hook Activity Tracking (CURRENT IMPLEMENTATION)

Use multiple hooks to update sentinel file frequently:

```rust
// Hooks that update thinking status:
- UserPromptSubmit: user submitted new prompt ✅ IMPLEMENTED (added thinking touch)
- PreToolUse: about to use a tool ✅ IMPLEMENTED
- ToolComplete: tool finished (could add this)
- Stop: done thinking ✅ IMPLEMENTED
```

All hooks just `touch` the file, updating its mtime. Our check looks at mtime, not existence.

**Benefits:**
- ✅ Multiple touch points = more frequent updates = better accuracy
- ✅ Works with Solution A for timing
- ✅ Still resilient to crashes (old files show as idle)

**Status**: IMPLEMENTED - current hooks provide good coverage

---

### Solution D: Hybrid Approach (Hooks + Minimal Polling) (OPTIONAL)

Use hooks for most cases, but add fallback polling for Claude sessions.

**Status**: NOT IMPLEMENTED - current hook-based approach is sufficient

## Implementation Summary

### ✅ Phase 1: File Modification Time Checking (Solution A)
1. ✅ Changed `is_claude_thinking()` to check mtime instead of existence
2. ✅ Added cleanup of stale files on app startup
3. ✅ 2 second threshold for "thinking" status

### ✅ Phase 2: UserPromptSubmit Hook Enhancement (Solution B)
1. ✅ Added thinking touch command to `UserPromptSubmit` hook
2. ✅ Captures initial thinking phase before tool use
3. ✅ Hooks fire in order: UserPromptSubmit → PreToolUse → Stop

### Combined Benefits
- ✅ Early detection: Start hook catches initial thinking
- ✅ Continuous updates: PreToolUse/Stop hooks update during tool use
- ✅ Crash resilience: mtime checking prevents false positives from stale files
- ✅ Automatic cleanup: Old files removed on startup
- ✅ No polling overhead: No tmux `capture_pane` calls for Claude

## Files Modified

- ✅ `src/app/sync.rs`
  - Updated `is_claude_thinking()` to check modification time
  - Added `cleanup_stale_thinking_files()` public function
- ✅ `src/app/setup.rs`
  - Added thinking touch command to `UserPromptSubmit` hook
  - Removed duplicate `UserPromptSubmit` hook insertion
  - Removed invalid `Start` hook (not supported by Claude Code)
- ✅ `src/main.rs`
  - Call `cleanup_stale_thinking_files()` on startup
- ✅ `performance-test/.claude/settings.json`
  - Manually cleaned up invalid "Start" hook
  - Added thinking touch to `UserPromptSubmit` hook

## Testing Recommendations

1. Test normal flow: Start Claude, submit a prompt, verify "thinking" shows immediately
2. Test tool usage: Verify "thinking" stays active during Edit/Write operations
3. Test completion: Verify "thinking" clears when Claude finishes responding
4. Test crash recovery: Kill Claude, verify stale file cleaned on next AMF start
5. Test multiple sessions: Run multiple Claude instances, verify independent thinking detection
6. ✅ Test settings validity: Verify settings.json has no invalid hooks (no "Start" hook)

## Current Hook Implementation

The thinking detection now uses three hooks:

1. **UserPromptSubmit** - Fires when user submits a prompt
   - Touches `/tmp/amf-thinking/{session}` file
   - Also saves the prompt text
   - This catches initial thinking/planning phase

2. **PreToolUse** - Fires when Claude starts using a tool (Edit, Write, etc.)
   - Touches `/tmp/amf-thinking/{session}` file
   - Also clears notifications
   - This maintains "thinking" status during tool operations

3. **Stop** - Fires when Claude finishes or is stopped
   - Removes `/tmp/amf-thinking/{session}` file
   - Also writes notification
   - This clears "thinking" status

Combined with the 2-second modification time threshold, this provides:
- ✅ Early detection (UserPromptSubmit)
- ✅ Continuous updates (PreToolUse)
- ✅ Proper cleanup (Stop)
- ✅ Crash resilience (mtime checking)
- ✅ Automatic cleanup of stale files on startup

