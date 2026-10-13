/**
 * RETIRED — superseded by the native aspect lock in src-tauri/src/lib.rs
 * (on_window_event Resized handler). The JS version raced the OS drag loop:
 * overlapping async corrections fought the live resize and could run away.
 * Kept as an empty stub so old bundles referencing it don't 404.
 */
