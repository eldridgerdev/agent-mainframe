#include <stdbool.h>
#include <stdio.h>

typedef struct Widget {
    const char *name;
    int count;
    bool enabled;
} Widget;

static int score_widget(const Widget *widget) {
    return widget->enabled ? widget->count * 2 : 0;
}

int main(void) {
    Widget widget = {.name = "syntax", .count = 7, .enabled = true};
    printf("%s => %d\n", widget.name, score_widget(&widget));
    return 0;
}
