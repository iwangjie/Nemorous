package plugins

import (
	"context"
	"database/sql"
	"encoding/base64"
	"fmt"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/dop251/goja"
	_ "github.com/go-sql-driver/mysql"
	_ "github.com/lib/pq"
	_ "github.com/sijms/go-ora/v2"
	_ "modernc.org/sqlite"
)

const (
	maxDBRows    = 2000
	maxDBArgs    = 256
	maxDBSQLLen  = 1 << 20
	maxDBCellLen = 64 << 10
)

type dbOptions struct {
	driver  string
	dsn     string
	sql     string
	args    []any
	mode    string
	timeout time.Duration
}

type dbResult struct {
	Columns      []string `json:"columns"`
	Rows         [][]any  `json:"rows"`
	RowCount     int      `json:"rowCount"`
	DurationMs   int64    `json:"durationMs"`
	Truncated    bool     `json:"truncated"`
	RowsAffected int64    `json:"rowsAffected,omitempty"`
	LastInsertID int64    `json:"lastInsertId,omitempty"`
}

func (rt *Runtime) startDatabase(call goja.FunctionCall, mode string) {
	if !rt.plugin.Manifest.HasPermission(PermissionDatabase) {
		panic(rt.vm.NewGoError(fmt.Errorf("插件未声明 database 权限")))
	}
	raw := call.Argument(0)
	if raw == nil || goja.IsUndefined(raw) || goja.IsNull(raw) {
		panic(rt.vm.NewGoError(fmt.Errorf("db: options object required")))
	}
	obj := raw.ToObject(rt.vm)
	if obj == nil {
		panic(rt.vm.NewGoError(fmt.Errorf("db: options object required")))
	}
	exported, _ := obj.Export().(map[string]any)
	opts, err := parseDBOptions(exported)
	if err != nil {
		panic(rt.vm.NewGoError(err))
	}
	if mode == "exec" {
		opts.mode = "exec"
	}
	onOK := call.Argument(1)
	onErr := call.Argument(2)
	go func() {
		result, err := runDBQuery(opts)
		if err != nil {
			rt.post(func() {
				if onErr != nil && !goja.IsUndefined(onErr) && !goja.IsNull(onErr) {
					rt.callJob(onErr, err.Error())
				} else {
					rt.report("db", err)
				}
				rt.markDirty()
			})
			return
		}
		rt.post(func() {
			rt.callJob(onOK, result)
			rt.markDirty()
		})
	}()
}

func parseDBOptions(exported map[string]any) (dbOptions, error) {
	var opts dbOptions
	if v, ok := exported["driver"].(string); ok {
		opts.driver = strings.ToLower(strings.TrimSpace(v))
	}
	switch opts.driver {
	case "mysql", "postgres", "sqlite", "oracle":
	default:
		return opts, fmt.Errorf("db: unsupported driver")
	}
	if v, ok := exported["dsn"].(string); ok {
		opts.dsn = strings.TrimSpace(v)
	}
	if opts.dsn == "" {
		return opts, fmt.Errorf("db: dsn is required")
	}
	if v, ok := exported["sql"].(string); ok {
		opts.sql = strings.TrimSpace(v)
	}
	if opts.sql == "" {
		return opts, fmt.Errorf("db: sql is required")
	}
	if len(opts.sql) > maxDBSQLLen {
		return opts, fmt.Errorf("db: sql is too large")
	}
	if v, ok := exported["mode"].(string); ok {
		opts.mode = strings.ToLower(strings.TrimSpace(v))
	}
	if opts.mode == "" {
		opts.mode = "query"
	}
	if opts.mode != "query" && opts.mode != "exec" {
		return opts, fmt.Errorf("db: mode must be query or exec")
	}
	if raw, ok := exported["args"].([]any); ok {
		if len(raw) > maxDBArgs {
			return opts, fmt.Errorf("db: too many arguments")
		}
		for _, a := range raw {
			if s, ok := a.(string); ok && len(s) > maxDBCellLen {
				return opts, fmt.Errorf("db: argument is too large")
			}
			opts.args = append(opts.args, a)
		}
	}
	ms := int64(20000)
	switch v := exported["timeoutMs"].(type) {
	case int64:
		ms = v
	case float64:
		ms = int64(v)
	}
	if ms < 1000 {
		ms = 1000
	}
	if ms > 120000 {
		ms = 120000
	}
	opts.timeout = time.Duration(ms) * time.Millisecond
	return opts, nil
}

func runDBQuery(opts dbOptions) (dbResult, error) {
	start := time.Now()
	ctx, cancel := context.WithTimeout(context.Background(), opts.timeout)
	defer cancel()
	dsn := opts.dsn
	if opts.driver == "sqlite" {
		dsn = sqliteDSN(dsn)
	}
	db, err := sql.Open(opts.driver, dsn)
	if err != nil {
		return dbResult{}, err
	}
	defer db.Close()
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(0)

	if opts.mode == "exec" {
		res, err := db.ExecContext(ctx, opts.sql, opts.args...)
		if err != nil {
			return dbResult{}, err
		}
		affected, _ := res.RowsAffected()
		lastID, _ := res.LastInsertId()
		return dbResult{
			Columns:      []string{},
			Rows:         [][]any{},
			RowsAffected: affected,
			LastInsertID: lastID,
			DurationMs:   time.Since(start).Milliseconds(),
		}, nil
	}

	rows, err := db.QueryContext(ctx, opts.sql, opts.args...)
	if err != nil {
		return dbResult{}, err
	}
	defer rows.Close()
	columns, err := rows.Columns()
	if err != nil {
		return dbResult{}, err
	}
	out := dbResult{Columns: columns, Rows: make([][]any, 0), DurationMs: 0}
	values := make([]any, len(columns))
	ptrs := make([]any, len(columns))
	for i := range values {
		ptrs[i] = &values[i]
	}
	for rows.Next() {
		if len(out.Rows) >= maxDBRows {
			out.Truncated = true
			break
		}
		if err := rows.Scan(ptrs...); err != nil {
			return dbResult{}, err
		}
		row := make([]any, len(values))
		for i, v := range values {
			row[i] = normalizeDBValue(v)
		}
		out.Rows = append(out.Rows, row)
	}
	if err := rows.Err(); err != nil {
		return dbResult{}, err
	}
	out.RowCount = len(out.Rows)
	out.DurationMs = time.Since(start).Milliseconds()
	return out, nil
}

func sqliteDSN(dsn string) string {
	if strings.HasPrefix(dsn, "file:") {
		return dsn
	}
	// SQLite interprets a bare Windows drive as a URI scheme; use its file:
	// form so `C:\data\app.db` opens instead of failing with SQLITE_CANTOPEN.
	if len(dsn) >= 2 && dsn[1] == ':' {
		return "file:" + strings.ReplaceAll(filepath.ToSlash(dsn), "\\", "/")
	}
	if !filepath.IsAbs(dsn) {
		return dsn
	}
	return "file:" + filepath.ToSlash(dsn)
}

func normalizeDBValue(v any) any {
	switch value := v.(type) {
	case nil:
		return nil
	case []byte:
		if len(value) > maxDBCellLen {
			value = value[:maxDBCellLen]
		}
		if utf8.Valid(value) {
			return string(value)
		}
		return "base64:" + base64.StdEncoding.EncodeToString(value)
	case time.Time:
		return value.Format(time.RFC3339Nano)
	case int64, float64, bool, string:
		if s, ok := value.(string); ok && len(s) > maxDBCellLen {
			return s[:maxDBCellLen]
		}
		return value
	default:
		s := fmt.Sprint(value)
		if len(s) > maxDBCellLen {
			return s[:maxDBCellLen]
		}
		return s
	}
}
