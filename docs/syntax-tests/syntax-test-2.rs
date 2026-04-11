#[derive(Debug, Clone)]
struct Widget<'a> {
    name: &'a str,
    count: usize,
}

impl Widget<'_> {
    fn label(&self) -> String {
        format!("{}:{}", self.name, self.count)
    }
}

fn main() {
    let widget = Widget {
        name: "syntax",
        count: 6,
    };

    println!("{}", widget.label());
}
