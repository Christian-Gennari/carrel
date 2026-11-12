//! One place that knows which screen is showing and how to go back.
//!
//! Pages never hold each other: they hold a [`Navigator`], which owns the
//! stack and a weak handle to the editor. Esc from any page calls
//! `show_editor`; the editor keeps focus so typing continues immediately.

use std::cell::RefCell;
use std::rc::{Rc, Weak};

use crate::editor::EditorPage;

pub struct Navigator {
    stack: gtk::Stack,
    editor: RefCell<Weak<EditorPage>>,
}

impl Navigator {
    pub fn new(stack: gtk::Stack) -> Rc<Self> {
        Rc::new(Self {
            stack,
            editor: RefCell::new(Weak::new()),
        })
    }

    pub fn attach(&self, editor: &Rc<EditorPage>) {
        *self.editor.borrow_mut() = Rc::downgrade(editor);
    }

    pub fn stack(&self) -> gtk::Stack {
        self.stack.clone()
    }

    pub fn show(&self, name: &str) {
        self.stack.set_visible_child_name(name);
    }

    pub fn show_editor(&self) {
        self.show("editor");
        if let Some(editor) = self.editor.borrow().upgrade() {
            editor.focus_editor();
        }
    }

    pub fn editor(&self) -> Option<Rc<EditorPage>> {
        self.editor.borrow().upgrade()
    }
}
