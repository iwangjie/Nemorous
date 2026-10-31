// Package credentials manages Git HTTPS credentials through the system's Git
// credential helper, plus SSH keys and agent entries. Secrets are handed to
// git on stdin and are never written to Nemorous configuration.
package credentials

import (
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// HTTPSHost normalizes a host or URL to the host key Git credentials use.
func HTTPSHost(raw string) string {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return ""
	}
	if i := strings.Index(raw, "://"); i >= 0 {
		raw = raw[i+3:]
	}
	scpLike := !strings.Contains(raw, "://") && strings.Contains(raw, "@")
	if i := strings.Index(raw, "://"); i >= 0 {
		raw = raw[i+3:]
	}
	if i := strings.LastIndex(raw, "@"); i >= 0 {
		raw = raw[i+1:]
	}
	cut := "/?#"
	if scpLike {
		cut += ":"
	}
	if i := strings.IndexAny(raw, cut); i >= 0 {
		raw = raw[:i]
	}
	return raw
}

func validateField(name, value string) error {
	if strings.ContainsAny(value, "\r\n\x00") {
		return fmt.Errorf("%s 不能包含换行", name)
	}
	return nil
}

func gitCredential(ctx context.Context, action, host, username, secret string) (string, error) {
	host = HTTPSHost(host)
	if host == "" {
		return "", fmt.Errorf("主机不能为空")
	}
	if username != "" {
		if err := validateField("用户名", username); err != nil {
			return "", err
		}
	}
	if secret != "" {
		if err := validateField("令牌", secret); err != nil {
			return "", err
		}
	}
	helper, args, err := credentialHelperCommand(ctx)
	if err != nil {
		return "", err
	}
	payload := fmt.Sprintf("protocol=https\nhost=%s\n", host)
	if username != "" {
		payload += "username=" + username + "\n"
	}
	if secret != "" {
		payload += "password=" + secret + "\n"
	}
	payload += "\n"
	helperAction := "store"
	if action == "reject" {
		helperAction = "erase"
	}
	cmdArgs := append(append([]string{}, args...), helperAction)
	cctx, cancel := context.WithTimeout(ctx, 15*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "git", append([]string{"credential-" + helper}, cmdArgs...)...)
	cmd.Stdin = strings.NewReader(payload)
	var out, stderr bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		msg := strings.TrimSpace(stderr.String())
		if msg == "" {
			msg = err.Error()
		}
		return "", fmt.Errorf("凭据 helper %s %s: %s", helper, helperAction, msg)
	}
	return strings.TrimSpace(out.String()), nil
}

// credentialHelperCommand picks the last persistent helper configured in Git.
// Calling the helper directly avoids running every system helper (some, such
// as a keychain prompt, can block a background refresh).
func credentialHelperCommand(ctx context.Context) (string, []string, error) {
	cctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "git", "config", "--get-all", "credential.helper")
	out, err := cmd.Output()
	if err != nil {
		return "", nil, fmt.Errorf("没有配置 Git credential.helper，请在「设置」或 git config 中配置 osxkeychain / manager / store")
	}
	var chosen string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		line = strings.TrimSpace(line)
		if line == "" || line == "cache" || strings.HasPrefix(line, "!") {
			continue
		}
		chosen = line
	}
	if chosen == "" {
		return "", nil, fmt.Errorf("没有可持久化的 Git credential.helper（cache 只保存在内存）")
	}
	fields := strings.Fields(chosen)
	name := fields[0]
	if strings.HasPrefix(name, "credential-") {
		name = strings.TrimPrefix(name, "credential-")
	}
	return name, fields[1:], nil
}

// SaveHTTPS stores an HTTPS token through Git's configured credential helper.
// On macOS this is normally the login Keychain.
func SaveHTTPS(ctx context.Context, host, username, token string) error {
	if strings.TrimSpace(token) == "" {
		return fmt.Errorf("令牌不能为空")
	}
	_, err := gitCredential(ctx, "approve", host, username, token)
	return err
}

// EraseHTTPS removes a stored HTTPS credential through the helper.
func EraseHTTPS(ctx context.Context, host, username string) error {
	_, err := gitCredential(ctx, "reject", host, username, "")
	return err
}

// CredentialHelpers lists the configured Git credential helpers.
func CredentialHelpers(ctx context.Context) []string {
	cctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "git", "config", "--get-all", "credential.helper")
	out, err := cmd.Output()
	if err != nil {
		return nil
	}
	var helpers []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line = strings.TrimSpace(line); line != "" {
			helpers = append(helpers, line)
		}
	}
	return helpers
}

// SSHKey is one public key in ~/.ssh.
type SSHKey struct {
	Path        string
	PublicPath  string
	Type        string
	Fingerprint string
	Comment     string
}

// ListSSHKeys lists public keys and their fingerprints.
func ListSSHKeys(ctx context.Context) ([]SSHKey, error) {
	home, err := os.UserHomeDir()
	if err != nil {
		return nil, err
	}
	dir := filepath.Join(home, ".ssh")
	entries, err := os.ReadDir(dir)
	if err != nil {
		if os.IsNotExist(err) {
			return nil, nil
		}
		return nil, err
	}
	var keys []SSHKey
	for _, ent := range entries {
		if ent.IsDir() || !strings.HasSuffix(ent.Name(), ".pub") {
			continue
		}
		pub := filepath.Join(dir, ent.Name())
		key := SSHKey{
			Path:       strings.TrimSuffix(pub, ".pub"),
			PublicPath: pub,
		}
		if data, err := os.ReadFile(pub); err == nil {
			fields := strings.Fields(string(data))
			if len(fields) >= 1 {
				key.Type = fields[0]
			}
			if len(fields) >= 3 {
				key.Comment = strings.Join(fields[2:], " ")
			}
		}
		if out, err := sshKeygen(ctx, "-lf", pub); err == nil {
			// 256 SHA256:... comment (ED25519)
			fields := strings.Fields(out)
			if len(fields) >= 2 {
				key.Fingerprint = fields[1]
			}
		}
		keys = append(keys, key)
	}
	sort.Slice(keys, func(i, j int) bool { return keys[i].Path < keys[j].Path })
	return keys, nil
}

func sshKeygen(ctx context.Context, args ...string) (string, error) {
	cctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "ssh-keygen", args...)
	var out, stderr bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("%s", strings.TrimSpace(stderr.String()))
	}
	return strings.TrimSpace(out.String()), nil
}

// GenerateSSHKey creates an ed25519 key in ~/.ssh and returns its public key.
func GenerateSSHKey(ctx context.Context, name, comment, passphrase string) (string, error) {
	name = strings.TrimSpace(name)
	if name == "" || strings.ContainsAny(name, `/\`) || strings.HasPrefix(name, ".") {
		return "", fmt.Errorf("无效的密钥文件名")
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return "", err
	}
	dir := filepath.Join(home, ".ssh")
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	path := filepath.Join(dir, name)
	if _, err := os.Stat(path); err == nil {
		return "", fmt.Errorf("密钥已存在：%s", path)
	}
	if comment = strings.TrimSpace(comment); comment == "" {
		comment = "nemorous"
	}
	cctx, cancel := context.WithTimeout(ctx, 30*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "ssh-keygen", "-t", "ed25519", "-f", path, "-N", passphrase, "-C", comment)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	if err := cmd.Run(); err != nil {
		return "", fmt.Errorf("生成密钥失败：%s", strings.TrimSpace(stderr.String()))
	}
	data, err := os.ReadFile(path + ".pub")
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(data)), nil
}

// AgentKeys lists keys currently loaded in ssh-agent.
func AgentKeys(ctx context.Context) ([]string, error) {
	cctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "ssh-add", "-l")
	out, err := cmd.Output()
	if err != nil {
		// Exit 1 means the agent is reachable but has no identities.
		return nil, nil
	}
	var keys []string
	for _, line := range strings.Split(strings.TrimSpace(string(out)), "\n") {
		if line = strings.TrimSpace(line); line != "" {
			keys = append(keys, line)
		}
	}
	return keys, nil
}

// TestSSH runs a non-interactive SSH authentication check for a Git host.
// GitHub and GitLab return exit status 1 even on success, so callers should
// display the message rather than treat the error alone as failure.
func TestSSH(ctx context.Context, host string) (string, error) {
	host = strings.TrimSpace(host)
	if host == "" {
		host = "github.com"
	}
	if strings.ContainsAny(host, " \t\r\n/") {
		return "", fmt.Errorf("无效的主机")
	}
	cctx, cancel := context.WithTimeout(ctx, 12*time.Second)
	defer cancel()
	cmd := exec.CommandContext(cctx, "ssh", "-T", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "git@"+host)
	var out bytes.Buffer
	cmd.Stdout = &out
	cmd.Stderr = &out
	err := cmd.Run()
	msg := strings.TrimSpace(out.String())
	if msg == "" && err != nil {
		msg = err.Error()
	}
	return msg, err
}
