// Command setversion stamps the tagged version into mygo.json before a
// release build, so the app and update manifests agree with the git tag.
package main

import (
	"encoding/json"
	"fmt"
	"os"
	"strings"
)

func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "usage: go run ./scripts/setversion v1.2.3")
		os.Exit(2)
	}
	version := strings.TrimPrefix(strings.TrimSpace(os.Args[1]), "v")
	if version == "" {
		fmt.Fprintln(os.Stderr, "empty version")
		os.Exit(2)
	}
	data, err := os.ReadFile("mygo.json")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	var cfg map[string]any
	if err := json.Unmarshal(data, &cfg); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	cfg["version"] = version
	out, err := json.MarshalIndent(cfg, "", "  ")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	if err := os.WriteFile("mygo.json", append(out, '\n'), 0o644); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
	fmt.Println("mygo.json version:", version)
}
