package main

import (
	"fmt"
	"os"
	"runtime"
)

func main() {
	fmt.Println("hello from go", runtime.GOOS, runtime.GOARCH, len(os.Args))
	os.Exit(7)
}
