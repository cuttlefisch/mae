# Heading 1 — Large Scale

Regular paragraph text for comparison.

## Heading 2 — Medium Scale

### Heading 3 — Small Scale

## Inline Formatting

This line has **bold text** and *italic text* and `inline code` together.

You can also combine **bold with `code` inside** or *italic with `code` inside*.

Here is ~~strikethrough text~~ in the middle of a sentence.

And ~~multiple strikethrough~~ words ~~in one line~~ for testing.

## Links

Markdown link: [MAE on GitHub](https://github.com/cuttlefisch/mae)

Another link: [Emacs source](https://git.savannah.gnu.org/cgit/emacs.git)

Bare URL (not concealed): https://example.com

## Code Blocks

Fenced code block with syntax highlighting:

```rust
fn main() {
    let greeting = "Hello, MAE!";
    println!("{}", greeting);
    for i in 0..10 {
        eprintln!("iteration {}", i);
    }
}
```

Another code block:

```python
def fibonacci(n):
    """Generate fibonacci sequence."""
    a, b = 0, 1
    for _ in range(n):
        yield a
        a, b = b, a + b

print(list(fibonacci(10)))
```

## Lists and Mixed Content

- Regular list item
- **Bold list item** with ~~strikethrough~~
- Item with `inline code` and *italics*
- Item with a [link](https://example.com) inside

## Checkboxes

- [ ] Unchecked item (press Enter to toggle)
- [x] Already checked item
- [ ] Another unchecked item

### Progress Cookies

Parent with fraction cookie [1/3]:
- [x] First task
- [ ] Second task
- [ ] Third task

Parent with percentage cookie [33%]:
- [x] Alpha
- [ ] Beta
- [ ] Gamma

## Inline Images

Default width (fits to text area):

![Test image](test-image.png)

Explicit width via attribute comment:

<!-- width=200 -->
![Small test image](test-image.png)

Curly-brace width attribute:

![Sized image](test-image.png){width=100}

Missing image (should show placeholder):

![Missing](does-not-exist.png)

## Everything Together

This paragraph has **bold**, *italic*, `code`, ~~strikethrough~~, and a
[concealed link](https://example.com) all in one place. The heading above
should have extra top padding, and code blocks should have a tinted background.
