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
	maxTableRows = 2000
	maxTableCols = 64
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
	BasePath    string `json:"basePath,omitempty"`
	Language    string `json:"language,omitempty"`
	Value       string `json:"value,omitempty"`
	Action      string `json:"action,omitempty"`
	Payload     any    `json:"payload,omitempty"`

	Items    []Item     `json:"items,omitempty"`
	Options  []string   `json:"options,omitempty"`
	Headers  []string   `json:"headers,omitempty"`
	Rows     [][]string `json:"rows,omitempty"`
	Children []*Node    `json:"children,omitempty"`

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
	Password  bool `json:"password,omitempty"`
	SoftWrap  bool `json:"softWrap,omitempty"`
	Vertical  bool `json:"vertical,omitempty"`
	Active    bool `json:"active,omitempty"`
	AutoFocus bool `json:"autoFocus,omitempty"`

	Fraction      float64 `json:"fraction,omitempty"`
	Width         float64 `json:"width,omitempty"`
	WidthPercent  float64 `json:"widthPercent,omitempty"`
	Height        float64 `json:"height,omitempty"`
	HeightPercent float64 `json:"heightPercent,omitempty"`
	MaxHeight     float64 `json:"maxHeight,omitempty"`
	Gap           float64 `json:"gap,omitempty"`
	Padding       float64 `json:"padding,omitempty"`
	Size          float64 `json:"size,omitempty"`
	Tone          string  `json:"tone,omitempty"`
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
	"input": true, "textarea": true, "codeeditor": true, "checkbox": true, "list": true, "code": true,
	"markdown": true, "table": true, "bar": true,
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
	textLimit := maxNodeText
	if n.Type == "markdown" || n.Type == "textarea" || n.Type == "codeeditor" {
		textLimit = 512 << 10
	}
	if n.Type == "markdown" {
		n.Text = trimTextHead(n.Text, textLimit)
	} else {
		n.Text = trimText(n.Text, textLimit)
	}
	n.Detail = trimText(n.Detail, 8<<10)
	n.Placeholder = trimTextHead(n.Placeholder, 200)
	n.BasePath = trimTextHead(n.BasePath, 4096)
	n.Language = trimTextHead(n.Language, 80)
	if n.Type == "textarea" || n.Type == "codeeditor" {
		n.Value = trimTextHead(n.Value, textLimit)
	} else {
		n.Value = trimText(n.Value, 4<<10)
	}
	n.Key = trimTextHead(n.Key, 512)
	n.Action = trimTextHead(n.Action, 120)
	*text += len(n.Text) + len(n.Detail) + len(n.Value)
	if *text > 4<<20 {
		return fmt.Errorf("render: too much text")
	}
	if n.Width < 0 || n.Width > 2000 || n.Height < 0 || n.Height > 4000 || n.MaxHeight < 0 || n.MaxHeight > 4000 {
		return fmt.Errorf("render: invalid size")
	}
	if n.WidthPercent < 0 || n.WidthPercent > 100 || n.HeightPercent < 0 || n.HeightPercent > 100 {
		return fmt.Errorf("render: invalid percentage size")
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
	if len(n.Headers) > maxTableCols {
		return fmt.Errorf("render: too many table columns")
	}
	for i := range n.Headers {
		n.Headers[i] = trimTextHead(n.Headers[i], 500)
		*text += len(n.Headers[i])
	}
	if len(n.Rows) > maxTableRows {
		return fmt.Errorf("render: too many table rows")
	}
	for i := range n.Rows {
		row := n.Rows[i]
		if len(row) > maxTableCols {
			return fmt.Errorf("render: too many table cells")
		}
		for j := range row {
			row[j] = trimTextHead(row[j], 4<<10)
			*text += len(row[j])
		}
		n.Rows[i] = row
	}
	if n.Fraction < 0 {
		n.Fraction = 0
	}
	if n.Fraction > 1 {
		n.Fraction = 1
	}
	if *text > 4<<20 {
		return fmt.Errorf("render: too much text")
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

func trimTextHead(s string, max int) string {
	if len(s) <= max {
		return s
	}
	cut := max
	for cut > 0 && !utf8.RuneStart(s[cut]) {
		cut--
	}
	return s[:cut] + "…"
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
