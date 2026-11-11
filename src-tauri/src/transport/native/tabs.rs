use tokio::task::AbortHandle;

#[derive(Default)]
pub struct Stream {
    id: u64,
    task: Option<AbortHandle>,
}

impl Stream {
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Cancels the running task and invalidates the id so late replies are ignored.
    pub fn stop(&mut self) {
        self.id += 1;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }

    pub fn start(&mut self, task: AbortHandle) {
        self.task = Some(task);
    }

    pub fn finish(&mut self) {
        self.task = None;
    }

    pub fn is_running(&self) -> bool {
        self.task.is_some()
    }
}

pub struct Tab<T> {
    pub id: u64,
    pub title: String,
    pub key: Option<String>,
    pub stream: Stream,
    pub view: T,
}

pub struct Tabs<T> {
    items: Vec<Tab<T>>,
    active: u64,
    next: u64,
}

impl<T> Tabs<T> {
    pub fn new(view: T) -> Self {
        let mut tabs = Self {
            items: Vec::new(),
            active: 0,
            next: 1,
        };
        tabs.open(None, None, view);
        tabs
    }

    pub fn items(&self) -> &[Tab<T>] {
        &self.items
    }

    pub fn active_id(&self) -> u64 {
        self.active
    }

    pub fn active(&self) -> &Tab<T> {
        let id = self.active;
        self.items
            .iter()
            .find(|tab| tab.id == id)
            .expect("the active tab exists")
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut Tab<T>> {
        self.items.iter_mut().find(|tab| tab.id == id)
    }

    pub fn active_mut(&mut self) -> &mut Tab<T> {
        let id = self.active;
        self.get_mut(id).expect("the active tab exists")
    }

    pub fn find(&self, key: &str) -> Option<u64> {
        let found = |tab: &&Tab<T>| tab.key.as_deref() == Some(key);
        self.items.iter().find(found).map(|tab| tab.id)
    }

    pub fn select(&mut self, id: u64) {
        if self.items.iter().any(|tab| tab.id == id) {
            self.active = id;
        }
    }

    /// Opens and selects a tab. Without a title it is named "Console N".
    pub fn open(&mut self, title: Option<String>, key: Option<String>, view: T) -> u64 {
        let id = self.next;
        self.next += 1;
        let title = title.unwrap_or_else(|| {
            let consoles = self.items.iter().filter(|tab| tab.key.is_none()).count();
            match consoles {
                0 => "Console".into(),
                n => format!("Console {}", n + 1),
            }
        });
        self.items.push(Tab {
            id,
            title,
            key,
            stream: Stream::default(),
            view,
        });
        self.active = id;
        id
    }

    /// Cancels the tab's stream and removes it. The last tab is replaced by `fresh()`.
    pub fn close(&mut self, id: u64, fresh: impl FnOnce() -> T) {
        let Some(index) = self.items.iter().position(|tab| tab.id == id) else {
            return;
        };
        self.items[index].stream.stop();
        self.items.remove(index);
        if self.items.is_empty() {
            self.open(None, None, fresh());
        } else if self.active == id {
            self.active = self.items[index.saturating_sub(1)].id;
        }
    }
}
