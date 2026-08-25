//! Native drag-out: drag a note or artifact from Alchemy into Finder, Mail,
//! or Messages as a real file (docs/RFC-professional-grade.md Pillar 6).
//!
//! Tauri v2 has no drag-source API — `startDragging` moves the *window*, and
//! `onDragDropEvent` is drag-destination only (that half is FileDrop.tsx).
//! So the drag session is opened directly against AppKit, on the same objc2
//! stack the Services provider already uses (services.rs).
//!
//! The file is written before the drag begins, so this hands AppKit a plain
//! `NSURL` rather than an `NSFilePromiseProvider`. That is the whole design:
//! a promise defers the write into a delegate callback that must then be
//! correct under drop-time conditions, whereas an already-written file is
//! just a URL the receiver copies. Nothing can half-succeed.
#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::runtime::NSObjectProtocol;
use objc2::runtime::ProtocolObject;
use objc2::{define_class, msg_send, AnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEventType, NSPasteboardWriting, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL,
};

define_class!(
    // SAFETY: NSObject has no subclassing requirements; no Drop impl.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[name = "AlchemyDragSource"]
    struct DragSource;

    unsafe impl NSObjectProtocol for DragSource {}

    unsafe impl NSDraggingSource for DragSource {
        /// Copy, never move — the dragged file is a temp export, and letting
        /// a receiver "move" it would delete the only copy out from under a
        /// note that still exists in the library.
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }
    }
);

impl DragSource {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

/// Begin dragging `path` out of the app. Must be called while the user is
/// actually dragging: AppKit needs the live mouse event to attach the
/// session to, which is why the frontend fires this from a mousedown-plus-
/// threshold gesture rather than from HTML5 dragstart.
pub fn start(path: &str) -> Result<(), String> {
    let Some(mtm) = MainThreadMarker::new() else {
        return Err("drag must start on the main thread".into());
    };
    if !std::path::Path::new(path).exists() {
        return Err(format!("nothing to drag at {path}"));
    }

    let app = NSApplication::sharedApplication(mtm);
    let window = app
        .keyWindow()
        .ok_or_else(|| "no key window to drag from".to_string())?;
    let view = window
        .contentView()
        .ok_or_else(|| "key window has no content view".to_string())?;
    // The drag rides the event the user is currently holding; without one
    // AppKit has no anchor and silently refuses to open a session.
    let event = app
        .currentEvent()
        .ok_or_else(|| "no current event to attach the drag to".to_string())?;
    // The session must hang off a live mouse drag. Slow exports (a poster
    // renders through the print pipeline) can outlast the gesture, and by
    // then the current event is a key-up or nothing at all. AppKit answers
    // that with nil, which the binding treats as non-null and panics on — so
    // check here, where it is a message rather than a crash.
    let kind = event.r#type();
    if !matches!(
        kind,
        NSEventType::LeftMouseDown | NSEventType::LeftMouseDragged
    ) {
        return Err("Let go too early — try dragging again".into());
    }

    let ns_path = NSString::from_str(path);
    let url = NSURL::fileURLWithPath(&ns_path);
    let writer: &ProtocolObject<dyn NSPasteboardWriting> = ProtocolObject::from_ref(&*url);
    let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);

    // Drag the file's own Finder icon, so the cursor shows what is moving
    // and the receiver's copy badge appears.
    let icon = NSWorkspace::sharedWorkspace().iconForFile(&ns_path);
    icon.setSize(NSSize::new(64.0, 64.0));
    let origin = view.convertPoint_fromView(event.locationInWindow(), None);
    let frame = NSRect::new(
        NSPoint::new(origin.x - 32.0, origin.y - 32.0),
        NSSize::new(64.0, 64.0),
    );
    // SAFETY: frame is a valid rect and the icon outlives the call.
    unsafe { item.setDraggingFrame_contents(frame, Some(&icon)) };

    let source = DragSource::new(mtm);
    let items = NSArray::from_retained_slice(&[item]);
    let session = view.beginDraggingSessionWithItems_event_source(
        &items,
        &event,
        ProtocolObject::from_ref(&*source),
    );
    // AppKit retains the session, but the source must outlive the drag and
    // nothing else holds it.
    std::mem::forget(source);
    let _ = session;
    crate::note!("dragout: started drag for {path}");
    Ok(())
}

/// IPC entry point. Errors cross as strings like every other command.
#[tauri::command]
pub fn start_file_drag(path: String) -> Result<(), String> {
    start(&path)
}

/// Export a note into a scratch directory and hand back the path, ready to
/// drag. Staging is a separate command from `start_file_drag` on purpose:
/// rendering a PDF or a deck takes long enough that doing it inside the drag
/// gesture would leave AppKit with a stale mouse event and no session. The
/// frontend stages on mouse-down and drags once the pointer actually moves.
#[tauri::command]
pub async fn stage_note_for_drag(
    app: tauri::AppHandle,
    note_id: String,
    format: String,
) -> Result<String, String> {
    use tauri::Manager;
    let dir = app
        .path()
        .temp_dir()
        .map_err(|e| format!("no temp dir: {e}"))?
        .join("alchemy-drag");
    // One scratch dir, rewritten per drag: these are throwaway copies of
    // notes that still live in the library, not user data to accumulate.
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not stage drag: {e}"))?;
    // export_note_file takes a full destination path, not a directory — and
    // the name it lands under is what the receiving app shows the user, so
    // it has to be the note's own title, exactly as the Save dialog would
    // have named it.
    let state = app.state::<crate::commands::AppState>();
    let note = state
        .db
        .get_note(&note_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no note with id {note_id}"))?;
    let ext = crate::export::export_ext(&format).map_err(|e| e.to_string())?;
    // Cached per note revision, under a directory keyed by id and edit
    // stamp so the visible filename stays the note's own title. A poster or
    // mind map renders through the print pipeline — a real window, a real
    // wait — and that cost belongs to the first drag only. Editing the note
    // changes the stamp, so a stale image can never be dragged.
    let dir = dir.join(format!("{note_id}-{}", note.updated_at));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not stage drag: {e}"))?;
    let dest = dir.join(format!("{}.{ext}", crate::export::safe_name(&note.title)));
    if dest.is_file() {
        return Ok(dest.to_string_lossy().into_owned());
    }
    crate::export::export_note_file(&app, &note_id, &format, Some(dest.to_string_lossy().into()))
        .await
        .map_err(|e| e.to_string())
}
