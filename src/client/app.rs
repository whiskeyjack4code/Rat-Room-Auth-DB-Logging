pub struct App {
    pub messages: Vec<String>,
    pub input: String,
    pub username: String,
    pub room: String,
    pub scroll: usize,
}

const MAX_MESSAGES: usize = 200;

impl App {
    pub fn new(username: String) -> Self {
        Self {
            messages: Vec::new(),
            input: String::new(),
            username,
            room: "lobby".to_string(),
            scroll: 0,
        }
    }

    pub fn push_message(&mut self, message: String) {
        self.messages.push(message);

        if self.messages.len() > MAX_MESSAGES {
            let overflow = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(0..overflow);
        }

        self.scroll_to_bottom();
    }

    pub fn scroll_up(&mut self) {
        if self.scroll > 0 {
            self.scroll -= 1;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll += 1;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll = self.messages.len();
    }
}
