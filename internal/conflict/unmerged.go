package conflict

import (
	"context"
	"strings"

	"github.com/iwangjie/nemorous/internal/gitops"
)

// Kind is the shape of an unmerged index entry, as git status reports it.
// The letters are git's XY code: X is ours, Y is theirs.
type Kind uint8

const (
	KindBothModified  Kind = iota // UU: both sides changed the file
	KindBothAdded                 // AA: both sides added the file
	KindDeletedByUs               // UD: ours deleted, theirs changed
	KindDeletedByThem             // DU: theirs deleted, ours changed
	KindAddedByUs                 // AU: ours added, theirs unmerged
	KindAddedByThem               // UA: theirs added, ours unmerged
	KindBothDeleted               // DD: both sides deleted
	KindUnknown
)

// Label is the short reason shown in the conflict file list.
func (k Kind) Label() string {
	switch k {
	case KindBothModified:
		return "双方修改"
	case KindBothAdded:
		return "双方新增"
	case KindDeletedByUs:
		return "我方删除"
	case KindDeletedByThem:
		return "对方删除"
	case KindAddedByUs:
		return "我方新增"
	case KindAddedByThem:
		return "对方新增"
	case KindBothDeleted:
		return "双方删除"
	default:
		return "未合并"
	}
}

// Detail explains a one-sided deletion, where opening the file shows no
// conflict markers even though the index is still unmerged.
func (k Kind) Detail() string {
	switch k {
	case KindDeletedByUs:
		return "我方删除了此文件，对方修改了它。"
	case KindDeletedByThem:
		return "对方删除了此文件，我方修改了它。"
	case KindBothDeleted:
		return "双方都删除了此文件。"
	case KindBothAdded:
		return "双方都新增了此文件，但没有冲突标记。"
	default:
		return "该文件在索引中仍未合并，但没有冲突标记。"
	}
}

// DeletesFile reports whether resolving this conflict can mean keeping the
// deletion instead of one side's content.
func (k Kind) DeletesFile() bool {
	return k == KindDeletedByUs || k == KindDeletedByThem || k == KindBothDeleted
}

// UnmergedFile is one unmerged index entry with its reason.
type UnmergedFile struct {
	Path string
	Kind Kind
	Code string
}

// UnmergedEntries lists unmerged files with git's conflict reason. Unlike
// UnresolvedFiles it keeps UD/DU/AA/DD rows, which have no conflict markers
// and would otherwise look like an already resolved file.
func UnmergedEntries(ctx context.Context, repoPath string) ([]UnmergedFile, error) {
	out, err := gitops.RunGitRaw(ctx, repoPath, "status", "--porcelain=v1", "-z", "--untracked-files=no")
	if err != nil {
		return nil, err
	}
	var files []UnmergedFile
	for _, rec := range strings.Split(out, "\x00") {
		if len(rec) < 4 {
			continue
		}
		code := rec[:2]
		kind, ok := kindForCode(code)
		if !ok {
			continue
		}
		files = append(files, UnmergedFile{
			Path: rec[3:],
			Kind: kind,
			Code: code,
		})
	}
	return files, nil
}

func kindForCode(code string) (Kind, bool) {
	switch code {
	case "UU":
		return KindBothModified, true
	case "AA":
		return KindBothAdded, true
	case "UD":
		return KindDeletedByThem, true
	case "DU":
		return KindDeletedByUs, true
	case "AU":
		return KindAddedByUs, true
	case "UA":
		return KindAddedByThem, true
	case "DD":
		return KindBothDeleted, true
	default:
		return KindUnknown, false
	}
}
