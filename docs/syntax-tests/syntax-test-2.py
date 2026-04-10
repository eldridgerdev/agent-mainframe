from dataclasses import dataclass


@dataclass(slots=True)
class Widget:
    name: str
    count: int

    def label(self) -> str:
        return f"{self.name}:{self.count}"


widget = Widget(name="syntax", count=8)
print(widget.label())
