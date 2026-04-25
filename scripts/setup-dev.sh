#!/usr/bin/env bash
# setup-dev.sh — install development dependencies for full self-test coverage
set -euo pipefail

OK="\033[32m✓\033[0m"
FAIL="\033[31m✗\033[0m"
WARN="\033[33m!\033[0m"

summary=()

check_cmd() {
    command -v "$1" &>/dev/null
}

install_lldb() {
    if check_cmd lldb-dap || check_cmd lldb-vscode; then
        echo -e "  $OK lldb-dap already installed"
        summary+=("lldb-dap: installed")
        return
    fi

    echo "  Installing lldb (provides lldb-dap)..."
    if check_cmd dnf; then
        sudo dnf install -y lldb
    elif check_cmd apt-get; then
        sudo apt-get install -y lldb
    elif check_cmd pacman; then
        sudo pacman -S --noconfirm lldb
    elif check_cmd brew; then
        brew install llvm
    else
        echo -e "  $FAIL Unknown package manager — install lldb manually"
        summary+=("lldb-dap: MISSING (install lldb manually)")
        return
    fi

    if check_cmd lldb-dap || check_cmd lldb-vscode; then
        echo -e "  $OK lldb-dap installed"
        summary+=("lldb-dap: installed")
    else
        echo -e "  $WARN lldb installed but lldb-dap not found in PATH"
        summary+=("lldb-dap: installed (check PATH)")
    fi
}

install_rust_analyzer() {
    if check_cmd rust-analyzer; then
        echo -e "  $OK rust-analyzer already installed"
        summary+=("rust-analyzer: installed")
        return
    fi

    echo "  Installing rust-analyzer via rustup..."
    if check_cmd rustup; then
        rustup component add rust-analyzer
        echo -e "  $OK rust-analyzer installed"
        summary+=("rust-analyzer: installed")
    else
        echo -e "  $FAIL rustup not found — install rust-analyzer manually"
        summary+=("rust-analyzer: MISSING")
    fi
}

install_debugpy() {
    if python3 -c "import debugpy" 2>/dev/null; then
        echo -e "  $OK debugpy already installed"
        summary+=("debugpy: installed")
        return
    fi

    echo "  Installing debugpy via pip..."
    if check_cmd pip3; then
        pip3 install --user debugpy
    elif check_cmd pip; then
        pip install --user debugpy
    else
        echo -e "  $FAIL pip not found — install debugpy manually (pip install debugpy)"
        summary+=("debugpy: MISSING")
        return
    fi

    if python3 -c "import debugpy" 2>/dev/null; then
        echo -e "  $OK debugpy installed"
        summary+=("debugpy: installed")
    else
        echo -e "  $WARN debugpy install attempted but import failed"
        summary+=("debugpy: install failed")
    fi
}

echo "MAE Development Dependencies"
echo "============================"
echo ""

echo "[1/3] lldb-dap (DAP adapter for C/C++/Rust)"
install_lldb
echo ""

echo "[2/3] rust-analyzer (LSP server for Rust)"
install_rust_analyzer
echo ""

echo "[3/3] debugpy (DAP adapter for Python)"
install_debugpy
echo ""

echo "============================"
echo "Summary:"
for item in "${summary[@]}"; do
    echo "  $item"
done
echo ""
echo "Override adapter paths with env vars:"
echo "  MAE_DAP_LLDB, MAE_DAP_CODELLDB, MAE_DAP_DEBUGPY"
echo "  MAE_LSP_RUST, MAE_LSP_PYTHON, MAE_LSP_TYPESCRIPT, MAE_LSP_GO"
