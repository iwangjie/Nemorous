package main

import (
	"context"
	"fmt"
	"image/png"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/egoist/mygo/ui"
	"github.com/iwangjie/nemorous/internal/config"
	"github.com/iwangjie/nemorous/internal/gitops"
)

func TestCaptureScreenshots(t *testing.T) {
	if os.Getenv("CAPTURE_SCREENS") != "1" {
		t.Skip("Set CAPTURE_SCREENS=1 to generate screenshots")
	}

	t.Log("Building realistic multi-repo test workspace...")
	ws := t.TempDir()

	// 1. Mani manifest
	maniContent := `projects:
  cloud-caring:
    path: cloud-caring
    desc: 心理测评与筛查核心微服务
  gateway:
    path: gateway
    desc: API 统一网关与鉴权中心
  risk-cloud:
    path: risk-cloud
    desc: 实时反欺诈与风控计算引擎
  monorepo:
    path: monorepo
    desc: 全栈共享核心模块与领域事件
  sby-libs:
    path: sby-libs
    desc: 业务公共 SDK 与第三方对接库
  order-center:
    path: order-center
    desc: 订单与交易履约服务
tasks:
  build:all:
    desc: 编译所有关联微服务
    cmd: go build ./...
  test:unit:
    desc: 并行运行全量单元测试
    cmd: go test -v ./...
  deploy:staging:
    desc: 构建镜像并部署至测试集群
    cmd: kubectl apply -k overlays/staging
`
	_ = os.WriteFile(filepath.Join(ws, "mani.yaml"), []byte(maniContent), 0o644)

	ctx := context.Background()

	type repoDef struct {
		name     string
		desc     string
		branch   string
		branches []string
	}

	repos := []repoDef{
		{"cloud-caring", "心理测评核心服务", "feat/ai-refactor", []string{"main", "feat/ai-refactor"}},
		{"gateway", "API 网关中心", "feat/ai-refactor", []string{"main", "feat/ai-refactor", "fix/cors"}},
		{"risk-cloud", "风控计算引擎", "main", []string{"main", "feat/ai-refactor"}},
		{"monorepo", "共享领域核心", "feat/ai-refactor", []string{"main", "feat/ai-refactor"}},
		{"sby-libs", "公共集成 SDK", "main", []string{"main", "feat/ai-refactor"}},
		{"order-center", "订单中心", "main", []string{"main"}},
	}

	authors := []struct {
		name  string
		email string
	}{
		{"Jie Wang", "wangjie@nemorous.dev"},
		{"Sarah Lin", "sarah@nemorous.dev"},
		{"Alex Chen", "alex@nemorous.dev"},
	}

	for repoIdx, r := range repos {
		rDir := filepath.Join(ws, r.name)
		_ = os.MkdirAll(rDir, 0o755)
		_, _ = gitops.RunGit(ctx, rDir, "init", "-b", "main")
		_, _ = gitops.RunGit(ctx, rDir, "config", "user.name", "Jie Wang")
		_, _ = gitops.RunGit(ctx, rDir, "config", "user.email", "wangjie@nemorous.dev")

		// Initial commits
		for cIdx := 0; cIdx < 4; cIdx++ {
			author := authors[(repoIdx+cIdx)%len(authors)]
			_, _ = gitops.RunGit(ctx, rDir, "config", "user.name", author.name)
			_, _ = gitops.RunGit(ctx, rDir, "config", "user.email", author.email)

			fName := "main.go"
			fContent := fmt.Sprintf("// Package main\npackage main\n\nimport \"fmt\"\n\n// Service: %s\nfunc Run%d() {\n\tfmt.Println(\"Running %s version %d\")\n}\n", r.name, cIdx, r.name, cIdx)

			if r.name == "cloud-caring" {
				_ = os.MkdirAll(filepath.Join(rDir, "docker", "k8s", "overlays", "alipay"), 0o755)
				_ = os.MkdirAll(filepath.Join(rDir, "internal", "handler"), 0o755)
				depYml := "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: caring-alipay-service\n  namespace: production\nspec:\n  replicas: 3\n  template:\n    spec:\n      containers:\n      - name: app\n        image: registry.internal/caring:v1.12.0\n        ports:\n        - containerPort: 8080\n"
				_ = os.WriteFile(filepath.Join(rDir, "docker", "k8s", "overlays", "alipay", "deployment.yml"), []byte(depYml), 0o644)

				handlerCode := "// Package handler implements assessment routing\npackage handler\n\nimport \"net/http\"\n\n// AssessmentHandler processes psychology questionnaires\nfunc AssessmentHandler(w http.ResponseWriter, r *http.Request) {\n\tw.WriteHeader(http.StatusOK)\n\t_, _ = w.Write([]byte(`{\"status\":\"ready\",\"version\":\"2.0\"}`)) // Return JSON response\n}\n"
				_ = os.WriteFile(filepath.Join(rDir, "internal", "handler", "assessment.go"), []byte(handlerCode), 0o644)
			} else if r.name == "sby-libs" {
				_ = os.MkdirAll(filepath.Join(rDir, "baidu-ocr-sdk"), 0o755)
				pom := "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>dev.nemorous.sdk</groupId>\n  <artifactId>baidu-ocr-sdk</artifactId>\n  <version>1.4.0</version>\n</project>\n"
				_ = os.WriteFile(filepath.Join(rDir, "baidu-ocr-sdk", "pom.xml"), []byte(pom), 0o644)
			}

			_ = os.WriteFile(filepath.Join(rDir, fName), []byte(fContent), 0o644)
			_, _ = gitops.RunGit(ctx, rDir, "add", "-A")

			msg := fmt.Sprintf("chore(%s): initial baseline setup #%d", r.name, cIdx+1)
			if cIdx == 1 {
				msg = fmt.Sprintf("feat(%s): integrate distributed trace context", r.name)
			} else if cIdx == 2 {
				msg = fmt.Sprintf("perf(%s): optimize memory alloc in worker pool", r.name)
			} else if cIdx == 3 {
				msg = fmt.Sprintf("fix(%s): resolve connection timeout under heavy load", r.name)
			}
			_, _ = gitops.RunGit(ctx, rDir, "commit", "-m", msg)
		}

		// Create other branches
		for _, b := range r.branches {
			if b != "main" {
				_, _ = gitops.RunGit(ctx, rDir, "branch", b)
			}
		}

		// Switch to active branch
		_, _ = gitops.RunGit(ctx, rDir, "checkout", r.branch)

		// Extra commit on feature branch
		if r.branch == "feat/ai-refactor" {
			_ = os.WriteFile(filepath.Join(rDir, "ai_pipeline.go"), []byte("// AI pipeline\npackage main\nfunc AIPipeline() {}\n"), 0o644)
			_, _ = gitops.RunGit(ctx, rDir, "add", "-A")
			_, _ = gitops.RunGit(ctx, rDir, "commit", "-m", fmt.Sprintf("feat(%s): implement agentic self-healing pipeline", r.name))
		}
	}

	// Make realistic dirty modifications!
	// 1. cloud-caring: deployment.yml modified
	newDepYml := "apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: caring-alipay-service\n  namespace: production\nspec:\n  replicas: 8\n  template:\n    spec:\n      containers:\n      - name: app\n        image: registry.internal/caring:v2.0.0-rc1\n        ports:\n        - containerPort: 9090\n        readinessProbe:\n          httpGet:\n            path: /healthz\n            port: 9090\n"
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "docker", "k8s", "overlays", "alipay", "deployment.yml"), []byte(newDepYml), 0o644)
	// Add a new file
	valuesProd := "# Production overrides\nreplicaCount: 8\nresources:\n  limits:\n    cpu: 4000m\n    memory: 8Gi\n"
	_ = os.WriteFile(filepath.Join(ws, "cloud-caring", "docker", "k8s", "overlays", "alipay", "values.prod.yaml"), []byte(valuesProd), 0o644)

	// 2. sby-libs: pom.xml modified
	newPom := "<project xmlns=\"http://maven.apache.org/POM/4.0.0\">\n  <modelVersion>4.0.0</modelVersion>\n  <groupId>dev.nemorous.sdk</groupId>\n  <artifactId>baidu-ocr-sdk</artifactId>\n  <version>2.0.0-SNAPSHOT</version>\n</project>\n"
	_ = os.WriteFile(filepath.Join(ws, "sby-libs", "baidu-ocr-sdk", "pom.xml"), []byte(newPom), 0o644)

	// 3. gateway: routes.yaml modified
	_ = os.WriteFile(filepath.Join(ws, "gateway", "routes.yaml"), []byte("routes:\n  - path: /api/v2/assessment\n    target: http://cloud-caring:9090\n"), 0o644)

	t.Log("Workspace populated. Starting Nemorous App in LIGHT mode...")

	saveScreen := func(img *ui.Tester, path string) {
		rgba := img.Image()
		_ = os.MkdirAll(filepath.Dir(path), 0o755)
		f, err := os.Create(path)
		if err != nil {
			t.Fatalf("failed to create %s: %v", path, err)
		}
		defer f.Close()
		if err := png.Encode(f, rgba); err != nil {
			t.Fatalf("failed to encode %s: %v", path, err)
		}
		t.Logf("Captured: %s (%dx%d)", path, rgba.Bounds().Dx(), rgba.Bounds().Dy())
	}

	cfg := config.Default()
	cfg.WorkspacePath = ws
	cfg.Theme = "light"
	cfg.IncludeRootRepo = false

	app := NewApp(cfg)
	app.systemDark = false
	app.LoadWorkspaceSync(ws)

	// Capture Screen 1: Changes & Diff View (⌘1)
	tt := ui.NewTester(app.BuildUI, 1440, 900)
	tt.SetDark(false)
	tt.Frame()
	time.Sleep(100 * time.Millisecond)
	tt.Frame()

	// Select cloud-caring deployment.yml
	for _, st := range app.statuses {
		for _, cf := range st.Files {
			if cf.Path == "docker/k8s/overlays/alipay/deployment.yml" {
				app.selectChangedFile(cf)
				break
			}
		}
	}
	tt.Frame()
	tt.Frame()
	saveScreen(tt, "docs/screenshots/01_changes_diff_light.png")

	// Capture Screen 2: Code Explorer & Blame (⌘2)
	app.activeTab = TabCode
	for _, wf := range app.workspaceFiles {
		if wf.RepoName == "cloud-caring" && wf.RelPath == "internal/handler/assessment.go" {
			app.selectCodeFile(wf)
			break
		}
	}
	if tab := app.activeCodeTab(); tab != nil {
		tab.blameAll = true
		tab.blameEnabled = true
	}
	tt.Frame()
	tt.Frame()
	saveScreen(tt, "docs/screenshots/02_code_explorer_light.png")

	// Capture Screen 3: Unified Commit Forest (⌘3)
	app.showGitPanel = true
	app.gitSubTab = "history"
	tt.Frame()
	tt.Frame()
	saveScreen(tt, "docs/screenshots/03_commit_forest_light.png")

	// Capture Screen 4: Branch Switcher Modal (⌘B)
	app.showGitPanel = false
	app.openBranchSwitchModal()
	tt.Frame()
	tt.Frame()
	saveScreen(tt, "docs/screenshots/04_branch_switch_light.png")

	// Capture Screen 5: Changes View in Dark Theme (for comparison)
	app.showBranchModal = false
	app.activeTab = TabChanges
	app.cfg.Theme = "dark"
	app.systemDark = true
	tt.SetDark(true)
	tt.Frame()
	tt.Frame()
	saveScreen(tt, "docs/screenshots/05_changes_dark.png")

	t.Log("All screenshots successfully captured!")
}
