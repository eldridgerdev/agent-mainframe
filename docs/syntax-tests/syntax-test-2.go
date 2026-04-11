package main

import "fmt"

type Widget struct {
	Name   string
	Active bool
	Count  int
}

func (w Widget) Label() string {
	return fmt.Sprintf("%s:%d", w.Name, w.Count)
}

func main() {
	widget := Widget{Name: "syntax", Active: true, Count: 4}
	fmt.Println(widget.Label())
}
