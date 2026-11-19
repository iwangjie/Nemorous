package editor

import (
	"sort"
	"strconv"

	"github.com/egoist/mygo/internal/diff"
	"github.com/egoist/mygo/ui"
)

// ThreeWayState keeps three source editors aligned on their middle version.
// Each State holds its own unmodified text and selection.
type ThreeWayState struct {
	Versions [3]*State
	// RenderAccept builds each visible change control into the middle editor.
	RenderAccept func(c *ui.Context, control ui.Element, fromLeft bool, apply func())
	models       [2]diffModel
	columns      [3]threeWayColumn
	scroll       sharedScroll
}

type threeWayPad struct{ line, rows int }

type threeWayColumn struct {
	editor *editor
	marks  [2][]lineSpan
	pads   []threeWayPad
	lh     float32
	lines  int
	blocks []block
}

func ThreeWayView(c *ui.Context, v *ThreeWayState, titles [3]string) ui.Element {
	for i, st := range v.Versions {
		if st.Buffer == nil {
			st.Buffer = NewBuffer("")
		}
		st.Buffer.watch(c.Services())
		v.columns[i].editor = &st.ed
	}
	v.models[0].set(v.Versions[0].Buffer, v.Versions[1].Buffer, false)
	v.models[1].set(v.Versions[2].Buffer, v.Versions[1].Buffer, false)
	left, right := v.models[0].update(), v.models[1].update()
	if left || right {
		v.align()
	}
	v.scroll.eds = []*editor{&v.Versions[0].ed, &v.Versions[1].ed, &v.Versions[2].ed}
	row := ui.Row(c).Clip()
	row.Children(func() {
		for i, st := range v.Versions {
			if i > 0 {
				ui.Box(c.Key("separator" + strconv.Itoa(i))).Width(1).Shrink(0).FillHeight().Background(c.Theme().Border)
			}
			key := c.Key("version" + strconv.Itoa(i))
			ui.Column(key).Width(0).Grow(1).FillHeight().Clip().Children(func() {
				ui.Text(key, titles[i]).Padding(5, 10).SingleLine().FontSize(11)
				opts := st.options()
				opts.decor, opts.sharedScroll = &v.columns[i], &v.scroll
				opts.noScrollbar = i < 2
				opts.wrap, opts.folding = false, false
				ui.Box(key.Key("pane")).FillWidth().Grow(1).Clip().Children(func() {
					st.ed.build(key, st.Buffer, opts).Fill()
					if i == 1 && v.RenderAccept != nil && !st.ReadOnly {
						v.buildAcceptControls(key, &st.ed)
					}
				})
			})
		}
	})
	return row
}

func (v *ThreeWayState) buildAcceptControls(c *ui.Context, middle *editor) {
	if middle.viewH <= 0 {
		return
	}
	first, last := middle.visibleLines(middle.scrollY, middle.scrollY+float64(middle.viewH))
	for side := range v.models {
		model := &v.models[side]
		if model.comparing() {
			continue
		}
		start := sort.Search(len(model.hunks), func(i int) bool { return model.hunks[i].B1 >= first })
		for i := start; i < len(model.hunks); i++ {
			h := model.hunks[i]
			if h.B0 > last+1 {
				break
			}
			line := min(h.B0, middle.view.text.LineCount()-1)
			top := float32(middle.rowTop(line) - middle.scrollY)
			if top < -middle.view.lineHeight || top > middle.viewH {
				continue
			}
			key := c.Key("accept-" + strconv.Itoa(side) + "-" + strconv.Itoa(i))
			control := ui.Row(key).Absolute().Top(top).KeepFocus()
			if side == 0 {
				control.Left(1)
			} else {
				control.Right(7)
			}
			v.RenderAccept(key, control, side == 0, func() {
				model.new.Edit(model.hunkEdit(h, model.oldText, model.newText))
			})
		}
	}
}

type alignmentSide struct {
	extra map[int]int
	gaps  []lineSpan
}

func buildAlignmentSide(hunks []diff.Hunk) alignmentSide {
	side := alignmentSide{extra: make(map[int]int)}
	for _, h := range hunks {
		old, middle := h.A1-h.A0, h.B1-h.B0
		if old > middle {
			side.extra[h.B1] += old - middle
		}
		if old < middle {
			side.gaps = append(side.gaps, lineSpan{h.B0 + old, h.B1 - 1})
		}
	}
	return side
}

func (v *ThreeWayState) align() {
	for i := range v.columns {
		col := &v.columns[i]
		col.pads, col.blocks, col.lh = col.pads[:0], nil, 0
		for j := range col.marks {
			col.marks[j] = col.marks[j][:0]
		}
	}
	var sides [2]alignmentSide
	for i, model := range v.models {
		if model.comparing() {
			return
		}
		sides[i] = buildAlignmentSide(model.hunks)
		for _, h := range model.hunks {
			if h.A1 > h.A0 {
				col := &v.columns[2*i]
				col.marks[0] = append(col.marks[0], lineSpan{h.A0, h.A1 - 1})
			}
			if h.B1 > h.B0 {
				col := &v.columns[1]
				col.marks[i] = append(col.marks[i], lineSpan{h.B0, h.B1 - 1})
			}
		}
	}
	if len(sides[0].extra)+len(sides[1].extra)+len(sides[0].gaps)+len(sides[1].gaps) == 0 {
		return
	}
	count := v.Versions[1].Buffer.LineCount()
	var cursors [3]int
	add := func(visible [3]bool) {
		for i, present := range visible {
			if present {
				cursors[i]++
			} else {
				v.columns[i].addPad(cursors[i])
			}
		}
	}
	for i := 0; i <= count; i++ {
		left, right := sides[0].extra[i], sides[1].extra[i]
		for j := 0; j < max(left, right); j++ {
			add([3]bool{j < left, false, j < right})
		}
		if i < count {
			add([3]bool{!containsAlignmentLine(sides[0].gaps, i), true, !containsAlignmentLine(sides[1].gaps, i)})
		}
	}
}

func containsAlignmentLine(spans []lineSpan, line int) bool {
	i := sort.Search(len(spans), func(i int) bool { return spans[i].to >= line })
	return i < len(spans) && spans[i].from <= line
}

func (col *threeWayColumn) addPad(line int) {
	if n := len(col.pads); n > 0 && col.pads[n-1].line == line {
		col.pads[n-1].rows++
		return
	}
	col.pads = append(col.pads, threeWayPad{line, 1})
}

func (col *threeWayColumn) displayBlocks(lines int, lh float32) []block {
	if col.lh == lh && col.lines == lines {
		return col.blocks
	}
	col.lines, col.lh = lines, lh
	col.blocks = col.blocks[:0]
	for _, pad := range col.pads {
		col.blocks = append(col.blocks, block{
			line: min(lines-1, pad.line), below: pad.line >= lines,
			height: float32(pad.rows) * lh, kind: blockSpacer,
		})
	}
	return col.blocks
}

func (col *threeWayColumn) lineStyle(line int) lineLook {
	if containsAlignmentLine(col.marks[0], line) || containsAlignmentLine(col.marks[1], line) {
		color := col.editor.theme.Modified
		return lineLook{bg: color.Alpha(0.15), gutter: color.Alpha(0.16), number: color, bar: color}
	}
	return lineLook{}
}

func (*threeWayColumn) words(int) []diffRange { return nil }
func (*threeWayColumn) number(line int) int   { return line + 1 }

func (*threeWayColumn) blockInput(*editor, *block, ui.InputEvent, ui.Rect) bool { return false }

func (*threeWayColumn) paintBlock(p *ui.Painter, ed *editor, _ *block, r ui.Rect, _ float64) {
	p.Fill(r, ed.theme.Background, 0)
}
