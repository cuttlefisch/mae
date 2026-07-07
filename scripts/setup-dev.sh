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

install_clangd() {
    if check_cmd clangd; then
        echo -e "  $OK clangd already installed"
        summary+=("clangd: installed")
        return
    fi

    echo "  Installing clangd (LSP server for C/C++)..."
    if check_cmd dnf; then
        sudo dnf install -y clang-tools-extra
    elif check_cmd apt-get; then
        sudo apt-get install -y clangd
    elif check_cmd pacman; then
        sudo pacman -S --noconfirm clang
    elif check_cmd brew; then
        brew install llvm
    else
        echo -e "  $FAIL Unknown package manager — install clangd manually"
        summary+=("clangd: MISSING (install clangd manually)")
        return
    fi

    if check_cmd clangd; then
        echo -e "  $OK clangd installed"
        summary+=("clangd: installed")
    else
        echo -e "  $WARN clangd installed but not found in PATH (may need PATH tweak)"
        summary+=("clangd: installed (check PATH)")
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

install_clang() {
    if check_cmd clang; then
        echo -e "  $OK clang already installed"
        summary+=("clang: installed")
        return
    fi

    echo "  Installing clang (required for GUI build — skia-safe)..."
    if check_cmd dnf; then
        sudo dnf install -y clang clang-devel
    elif check_cmd apt-get; then
        sudo apt-get install -y clang libclang-dev
    elif check_cmd pacman; then
        sudo pacman -S --noconfirm clang
    elif check_cmd brew; then
        echo -e "  $OK macOS Xcode CLI tools provide clang"
        summary+=("clang: provided by Xcode")
        return
    else
        echo -e "  $FAIL Unknown package manager — install clang manually"
        summary+=("clang: MISSING (install manually)")
        return
    fi

    if check_cmd clang; then
        echo -e "  $OK clang installed"
        summary+=("clang: installed")
    else
        echo -e "  $WARN clang install attempted but not found in PATH"
        summary+=("clang: install failed (check PATH)")
    fi
}

install_rustfmt_clippy() {
    local need=()
    check_cmd cargo-fmt || need+=("rustfmt")
    if ! cargo clippy --version &>/dev/null; then
        need+=("clippy")
    fi

    if [ ${#need[@]} -eq 0 ]; then
        echo -e "  $OK rustfmt + clippy already installed"
        summary+=("rustfmt/clippy: installed")
        return
    fi

    if check_cmd rustup; then
        echo "  Installing ${need[*]} via rustup..."
        rustup component add "${need[@]}"
    else
        # No rustup (e.g. Rust installed via the system package manager) —
        # fall back to distro packages, same pattern as the other installers.
        echo "  rustup not found — installing ${need[*]} via system package manager..."
        if check_cmd dnf; then
            sudo dnf install -y "${need[@]}"
        elif check_cmd apt-get; then
            sudo apt-get install -y "${need[@]}"
        elif check_cmd pacman; then
            sudo pacman -S --noconfirm "${need[@]}"
        elif check_cmd brew; then
            brew install "${need[@]}"
        else
            echo -e "  $FAIL Unknown package manager — install rustfmt/clippy manually"
            summary+=("rustfmt/clippy: MISSING (install manually)")
            return
        fi
    fi

    if check_cmd cargo-fmt && cargo clippy --version &>/dev/null; then
        echo -e "  $OK rustfmt + clippy installed"
        summary+=("rustfmt/clippy: installed")
    else
        echo -e "  $WARN install attempted but rustfmt/clippy still not found"
        summary+=("rustfmt/clippy: install failed (check PATH/toolchain)")
    fi
}

echo "MAE Development Dependencies"
echo "============================"
echo ""

echo "[1/6] rustfmt + clippy (required to run 'make fmt'/'make clippy' and the pre-commit hook)"
install_rustfmt_clippy
echo ""

echo "[2/6] clang (required for GUI build — skia-safe FFI)"
install_clang
echo ""

echo "[3/6] lldb-dap (DAP adapter for C/C++/Rust)"
install_lldb
echo ""

echo "[4/6] rust-analyzer (LSP server for Rust)"
install_rust_analyzer
echo ""

echo "[5/6] clangd (LSP server for C/C++)"
install_clangd
echo ""

echo "[6/6] debugpy (DAP adapter for Python)"
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
