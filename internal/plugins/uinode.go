package plugins

import (
	"encoding/json"
	"fmt"
	"strings"
	"unicode/utf8"
)

const (
	maxNodeCount = 4000
	maxNodeDepth = 24
	maxNodeText  = 220 << 10
	maxListItems = 600
	maxPayload   = 64 << 10
)

// Node is one element of the declarative UI a plugin's render() returns.
// The host draws these with the app's own widgets, so plugins cannot
// change colors or layout beyond the fields below.
type Node struct {
	Type        string `json:"type"`
	Key         string `json:"key,omitempty"`
	Text        string `json:"text,omitempty"`
	Detail      string `json:"detail,omitempty"`
	Placeholder string `json:"placeholder,omitempty"`
	Value       string `json:"value,omitempty"`
	Action      string `json:"action,omitempty"`
	Payload     any    `json:"payload,omitempty"`

	Items    []Item   `json:"items,omitempty"`
	Options  []string `json:"options,omitempty"`
	Children []*Node  `json:"children,omitempty"`

	Grow      bool `json:"grow,omitempty"`
	Scroll    bool `json:"scroll,omitempty"`
	Follow    bool `json:"follow,omitempty"`
	Mono      bool `json:"mono,omitempty"`
	Bold      bool `json:"bold,omitempty"`
	Muted     bool `json:"muted,omitempty"`
	Primary   bool `json:"primary,omitempty"`
	Small     bool `json:"small,omitempty"`
	Disabled  bool `json:"disabled,omitempty"`
	Checked   bool `json:"checked,omitempty"`
	Active    bool `json:"active,omitempty"`
	AutoFocus bool `json:"autoFocus,omitempty"`

	Width     float64 `json:"width,omitempty"`
	Height    float64 `json:"height,omitempty"`
	MaxHeight float64 `json:"maxHeight,omitempty"`
	Gap       float64 `json:"gap,omitempty"`
	Padding   float64 `json:"padding,omitempty"`
	Size      float64 `json:"size,omitempty"`
	Tone      string  `json:"tone,omitempty"`
}

// Item is a row of a list node.
type Item struct {
	Text     string `json:"text"`
	Detail   string `json:"detail,omitempty"`
	Depth    int    `json:"depth,omitempty"`
	Action   string `json:"action,omitempty"`
	Payload  any    `json:"payload,omitempty"`
	Tone     string `json:"tone,omitempty"`
	Selected bool   `json:"selected,omitempty"`
	Bold     bool   `json:"bold,omitempty"`
	Mono     bool   `json:"mono,omitempty"`
}

var nodeTypes = map[string]bool{
	"column": true, "row": true, "text": true, "button": true,
	"input": true, "checkbox": true, "list": true, "code": true,
	"badge": true, "divider": true, "spacer": true, "spinner": true,
}

var tones = map[string]bool{
	"": true, "accent": true, "ok": true, "warn": true, "err": true, "muted": true,
}

// decodeNode turns a goja export into a validated Node tree.
func decodeNode(v any) (*Node, error) {
	data, err := json.Marshal(v)
	if err != nil {
		return nil, fmt.Errorf("render: %w", err)
	}
	var n Node
	if err := json.Unmarshal(data, &n); err != nil {
		return nil, fmt.Errorf("render: %w", err)
	}
	count := 0
	text := 0
	if err := normalizeNode(&n, 0, &count, &text); err != nil {
		return nil, err
	}
	return &n, nil
}

func normalizeNode(n *Node, depth int, count, text *int) error {
	*count++
	if *count > maxNodeCount {
		return fmt.Errorf("render: too many nodes")
	}
	if depth > maxNodeDepth {
		return fmt.Errorf("render: tree is too deep")
	}
	if n == nil {
		return fmt.Errorf("render: null node")
	}
	if !nodeTypes[n.Type] {
		return fmt.Errorf("render: unknown node type %q", n.Type)
	}
	n.Text = trimText(n.Text, maxNodeText)
	n.Detail = trimText(n.Detail, 8<<10)
	n.Placeholder = trimText(n.Placeholder, 200)
	n.Value = trimText(n.Value, 4<<10)
	n.Key = trimText(n.Key, 120)
	n.Action = trimText(n.Action, 120)
	*text += len(n.Text) + len(n.Detail) + len(n.Value)
	if *text > 4<<20 {
		return fmt.Errorf("render: too much text")
	}
	if n.Width < 0 || n.Width > 2000 || n.Height < 0 || n.Height > 4000 || n.MaxHeight < 0 || n.MaxHeight > 4000 {
		return fmt.Errorf("render: invalid size")
	}
	if n.Gap < 0 || n.Gap > 64 {
		n.Gap = 0
	}
	if n.Padding < 0 || n.Padding > 64 {
		n.Padding = 0
	}
	if n.Size < 0 || n.Size > 40 {
		n.Size = 0
	}
	if !tones[n.Tone] {
		n.Tone = ""
	}
	if n.Payload != nil {
		payload, err := json.Marshal(n.Payload)
		if err != nil || len(payload) > maxPayload {
			return fmt.Errorf("render: payload too large")
		}
	}
	if len(n.Options) > 100 {
		return fmt.Errorf("render: too many options")
	}
	for i, opt := range n.Options {
		n.Options[i] = trimText(opt, 200)
	}
	if len(n.Items) > maxListItems {
		return fmt.Errorf("render: too many list items")
	}
	for i := range n.Items {
		it := &n.Items[i]
		it.Text = trimText(it.Text, maxNodeText)
		it.Detail = trimText(it.Detail, 8<<10)
		it.Action = trimText(it.Action, 120)
		if it.Depth < 0 {
			it.Depth = 0
		}
		if it.Depth > 40 {
			it.Depth = 40
		}
		if !tones[it.Tone] {
			it.Tone = ""
		}
		if it.Payload != nil {
			payload, err := json.Marshal(it.Payload)
			if err != nil || len(payload) > maxPayload {
				return fmt.Errorf("render: item payload too large")
			}
		}
	}
	if len(n.Children) > 200 {
		return fmt.Errorf("render: too many children")
	}
	for _, child := range n.Children {
		if err := normalizeNode(child, depth+1, count, text); err != nil {
			return err
		}
	}
	return nil
}

func trimText(s string, max int) string {
	if len(s) <= max {
		return s
	}
	// Cut at a rune boundary and keep the tail, which is what consoles
	// want.
	cut := len(s) - max
	for cut < len(s) && !utf8.RuneStart(s[cut]) {
		cut++
	}
	return "…" + s[cut:]
}

// errorNode is a panel's fallback when render() fails.
func errorNode(msg string) *Node {
	return &Node{
		Type: "column", Grow: true, Gap: 8, Padding: 12,
		Children: []*Node{
			{Type: "text", Text: "插件运行出错", Tone: "err", Bold: true, Size: 13},
			{Type: "text", Text: msg, Tone: "muted", Mono: true, Grow: true},
		},
	}
}

// emptyNode is what an unloaded panel shows.
func emptyNode(msg string) *Node {
	return &Node{
		Type: "column", Grow: true, Gap: 8, Padding: 12,
		Children: []*Node{
			{Type: "spinner", Text: msg},
		},
	}
}

// payloadSummary is used in error messages without exposing whole values.
func payloadSummary(p any) string {
	b, err := json.Marshal(p)
	if err != nil {
		return "?"
	}
	s := string(b)
	if len(s) > 80 {
		s = s[:80] + "…"
	}
	return strings.TrimSpace(s)
}
