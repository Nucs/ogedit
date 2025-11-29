#!/bin/bash
# Test script for file reload feature

echo "=== File Reload Feature Test ==="
echo

# Create test file
echo "Initial content - $(date)" > test_reload.txt
echo "✓ Created test_reload.txt"
echo

# Instructions
echo "STEP 1: Run the editor in a separate terminal:"
echo "  cargo run -- test_reload.txt"
echo

echo "STEP 2: Wait for the editor to open, then press Enter here..."
read -p "Press Enter when editor is open: "

# Modify the file
echo "External modification - $(date)" >> test_reload.txt
echo "✓ Modified test_reload.txt"
echo

echo "STEP 3: Look at the editor status bar (bottom)."
echo "  Within ~1 second, you should see a YELLOW button:"
echo "  [↻ Reload]"
echo

echo "STEP 4: Click the [↻ Reload] button to reload the file"
echo

echo "✓ Test complete!"
echo
echo "Check the logs at: ~/.ogedit/logs/"
echo "Look for: FILE_CHANGED_DETECTED and FILE_RELOADED entries"
