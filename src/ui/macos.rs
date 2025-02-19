#[cfg(target_os = "macos")]
use cocoa::{
    appkit::{NSApp, NSApplication, NSMenu, NSMenuItem},
    base::{id, nil, YES},
    foundation::{NSAutoreleasePool, NSString},
};
use objc::{
    class,
    declare::ClassDecl,
    msg_send,
    runtime::{Object, Sel, BOOL},
    sel, sel_impl,
};
use sciter::{make_args, Host};
use std::{
    ffi::c_void,
    rc::Rc,
    sync::{Arc, Mutex},
};

static APP_HANDLER_IVAR: &str = "GoDeskAppHandler";

const TERMINATE_TAG: u32 = 0;
const SHOW_ABOUT_TAG: u32 = 1;
const SHOW_SETTINGS_TAG: u32 = 2;
const RUN_ME_TAG: u32 = 3;

lazy_static::lazy_static! {
    pub static ref SHOULD_OPEN_UNTITLED_FILE_CALLBACK: Arc<Mutex<Option<Box<dyn Fn() + Send>>>> = Default::default();
}

trait AppHandler {
    fn command(&mut self, cmd: u32);
}

struct DelegateState {
    handler: Option<Box<dyn AppHandler>>,
}

impl DelegateState {
    fn command(&mut self, command: u32) {
        if command == TERMINATE_TAG {
            unsafe {
                let () = msg_send!(NSApp(), terminate: nil);
            }
        } else if let Some(inner) = self.handler.as_mut() {
            inner.command(command)
        }
    }
}

impl AppHandler for Rc<Host> {
    fn command(&mut self, cmd: u32) {
        if cmd == SHOW_ABOUT_TAG {
            let _ = self.call_function("showAbout", &make_args![]);
        } else if cmd == SHOW_SETTINGS_TAG {
            let _ = self.call_function("showSettings", &make_args![]);
        }
    }
}

// https://github.com/xi-editor/druid/blob/master/druid-shell/src/platform/mac/application.rs
unsafe fn set_delegate(handler: Option<Box<dyn AppHandler>>) {
    let mut decl =
        ClassDecl::new("AppDelegate", class!(NSObject)).expect("App Delegate definition failed");
    decl.add_ivar::<*mut c_void>(APP_HANDLER_IVAR);

    decl.add_method(
        sel!(applicationDidFinishLaunching:),
        application_did_finish_launching as extern "C" fn(&mut Object, Sel, id),
    );

    decl.add_method(
        sel!(applicationShouldOpenUntitledFile:),
        application_should_handle_open_untitled_file as extern "C" fn(&mut Object, Sel, id) -> BOOL,
    );

    decl.add_method(
        sel!(handleMenuItem:),
        handle_menu_item as extern "C" fn(&mut Object, Sel, id),
    );
    let decl = decl.register();
    let delegate: id = msg_send![decl, alloc];
    let () = msg_send![delegate, init];
    let state = DelegateState { handler };
    let handler_ptr = Box::into_raw(Box::new(state));
    (*delegate).set_ivar(APP_HANDLER_IVAR, handler_ptr as *mut c_void);
    let () = msg_send![NSApp(), setDelegate: delegate];
}

extern "C" fn application_did_finish_launching(_this: &mut Object, _: Sel, _notification: id) {
    unsafe {
        let () = msg_send![NSApp(), activateIgnoringOtherApps: YES];
    }
}

extern "C" fn application_should_handle_open_untitled_file(
    _this: &mut Object,
    _: Sel,
    _sender: id,
) -> BOOL {
    if let Some(callback) = SHOULD_OPEN_UNTITLED_FILE_CALLBACK.lock().unwrap().as_ref() {
        callback();
    }
    YES
}

/// This handles menu items in the case that all windows are closed.
extern "C" fn handle_menu_item(this: &mut Object, _: Sel, item: id) {
    unsafe {
        let tag: isize = msg_send![item, tag];
        let tag = tag as u32;
        if tag == RUN_ME_TAG {
            crate::run_me(Vec::<String>::new()).ok();
        } else {
            let inner: *mut c_void = *this.get_ivar(APP_HANDLER_IVAR);
            let inner = &mut *(inner as *mut DelegateState);
            (*inner).command(tag as u32);
        }
    }
}

unsafe fn make_menu_item(title: &str, key: &str, tag: u32) -> *mut Object {
    let title = NSString::alloc(nil).init_str(title);
    let action = sel!(handleMenuItem:);
    let key = NSString::alloc(nil).init_str(key);
    let object = NSMenuItem::alloc(nil)
        .initWithTitle_action_keyEquivalent_(title, action, key)
        .autorelease();
    let () = msg_send![object, setTag: tag];
    object
}

pub fn make_menubar(host: Rc<Host>) {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        set_delegate(Some(Box::new(host)));
        let menubar = NSMenu::new(nil).autorelease();
        let app_menu_item = NSMenuItem::new(nil).autorelease();
        menubar.addItem_(app_menu_item);
        let app_menu = NSMenu::new(nil).autorelease();

        if std::env::args().len() > 1 {
            let new_item = make_menu_item("New Window", "n", RUN_ME_TAG);
            app_menu.addItem_(new_item);
        } else {
            // When app launched without argument, is the main panel.
            let about_item = make_menu_item("About", "a", SHOW_ABOUT_TAG);
            app_menu.addItem_(about_item);
            let separator = NSMenuItem::separatorItem(nil).autorelease();
            app_menu.addItem_(separator);
            let settings_item = make_menu_item("Settings", "s", SHOW_SETTINGS_TAG);
            app_menu.addItem_(settings_item);
        }
        let separator = NSMenuItem::separatorItem(nil).autorelease();
        app_menu.addItem_(separator);
        let quit_item = make_menu_item(
            &format!("Quit {}", hbb_common::config::APP_NAME),
            "q",
            TERMINATE_TAG,
        );
        app_menu_item.setSubmenu_(app_menu);
        /*
        if !enabled {
            let () = msg_send![quit_item, setEnabled: NO];
        }

        if selected {
            let () = msg_send![quit_item, setState: 1_isize];
        }
        let () = msg_send![item, setTag: id as isize];
        */
        app_menu.addItem_(quit_item);
        NSApp().setMainMenu_(menubar);
    }
}
