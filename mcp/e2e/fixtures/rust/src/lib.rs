pub struct Widget {
    pub label: String,
}

impl Widget {
    pub fn render(&self) -> String {
        self.label.clone()
    }
}

pub fn make_widget(label: &str) -> Widget {
    Widget {
        label: label.to_string(),
    }
}

pub fn use_widget() -> String {
    let w = make_widget("x");
    w.render()
}
