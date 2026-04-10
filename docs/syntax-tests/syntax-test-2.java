import java.util.List;

record Widget(String name, int count) {}

public final class SyntaxTestTwo {
    public static void main(String[] args) {
        List<Widget> widgets = List.of(new Widget("syntax", 2), new Widget("diff", 5));

        widgets.stream()
            .map(widget -> widget.name() + ":" + widget.count())
            .forEach(System.out::println);
    }
}
