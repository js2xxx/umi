//! Copied and modified from [`relative-path`](https://github.com/udoprog/relative-path).

use alloc::{
    borrow::{Cow, ToOwned},
    boxed::Box,
    rc::Rc,
    string::String,
    sync::Arc,
};
use core::{
    borrow::Borrow,
    cmp, fmt,
    hash::{Hash, Hasher},
    iter::FromIterator,
    mem,
    ops::{self, Deref},
    str,
};

const STEM_SEP: char = '.';
const CURRENT_STR: &str = ".";
const PARENT_STR: &str = "..";

const SEP: char = '/';

fn split_file_at_dot(input: &str) -> (Option<&str>, Option<&str>) {
    if input == PARENT_STR {
        return (Some(input), None);
    }

    let mut iter = input.rsplitn(2, STEM_SEP);

    let after = iter.next();
    let before = iter.next();

    if before == Some("") {
        (Some(input), None)
    } else {
        (before, after)
    }
}

// Iterate through `iter` while it matches `prefix`; return `None` if `prefix`
// is not a prefix of `iter`, otherwise return `Some(iter_after_prefix)` giving
// `iter` after having exhausted `prefix`.
fn iter_after<'a, 'b, I, J>(mut iter: I, mut prefix: J) -> Option<I>
where
    I: Iterator<Item = Component<'a>> + Clone,
    J: Iterator<Item = Component<'b>>,
{
    loop {
        let mut iter_next = iter.clone();
        match (iter_next.next(), prefix.next()) {
            (Some(x), Some(y)) if x == y => (),
            (Some(_), Some(_)) => return None,
            (Some(_), None) => return Some(iter),
            (None, None) => return Some(iter),
            (None, Some(_)) => return None,
        }
        iter = iter_next;
    }
}

/// A single path component.
///
/// Accessed using the [Path::components] iterator.
///
/// # Examples
///
/// ```rust
/// use umifs::path::{Component, Path};
///
/// let path = Path::new("foo/../bar/./baz");
/// let mut it = path.components();
///
/// assert_eq!(Some(Component::Normal("foo")), it.next());
/// assert_eq!(Some(Component::ParentDir), it.next());
/// assert_eq!(Some(Component::Normal("bar")), it.next());
/// assert_eq!(Some(Component::CurDir), it.next());
/// assert_eq!(Some(Component::Normal("baz")), it.next());
/// assert_eq!(None, it.next());
/// ```
#[derive(Copy, Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum Component<'a> {
    /// The current directory `.`.
    CurDir,
    /// The parent directory `..`.
    ParentDir,
    /// A normal path component as a string.
    Normal(&'a str),
}

impl<'a> Component<'a> {
    /// Extracts the underlying [`str`][std::str] slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Path, Component};
    ///
    /// let path = Path::new("./tmp/../foo/bar.txt");
    /// let components: Vec<_> = path.components().map(Component::as_str).collect();
    /// assert_eq!(&components, &[".", "tmp", "..", "foo", "bar.txt"]);
    /// ```
    pub fn as_str(self) -> &'a str {
        use self::Component::*;

        match self {
            CurDir => CURRENT_STR,
            ParentDir => PARENT_STR,
            Normal(name) => name,
        }
    }
}

/// [AsRef<Path>] implementation for [Component].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let mut it = Path::new("../foo/bar").components();
///
/// let a = it.next().ok_or("a")?;
/// let b = it.next().ok_or("b")?;
/// let c = it.next().ok_or("c")?;
///
/// let a: &Path = a.as_ref();
/// let b: &Path = b.as_ref();
/// let c: &Path = c.as_ref();
///
/// assert_eq!(a, "..");
/// assert_eq!(b, "foo");
/// assert_eq!(c, "bar");
///
/// # Ok::<_, Box<dyn std::error::Error>>(())
/// ```
impl AsRef<Path> for Component<'_> {
    #[inline]
    fn as_ref(&self) -> &Path {
        self.as_str().as_ref()
    }
}

/// Traverse the given components and apply to the provided stack.
///
/// This takes '.', and '..' into account. Where '.' doesn't change the stack,
/// and '..' pops the last item or further adds parent components.
#[inline(always)]
fn relative_traversal<'a, C>(buf: &mut PathBuf, components: C)
where
    C: IntoIterator<Item = Component<'a>>,
{
    use self::Component::*;

    for c in components {
        match c {
            CurDir => (),
            ParentDir => match buf.components().next_back() {
                Some(Component::ParentDir) | None => {
                    buf.push(PARENT_STR);
                }
                _ => {
                    buf.pop();
                }
            },
            Normal(name) => {
                buf.push(name);
            }
        }
    }
}

/// Iterator over all the components in a path.
#[derive(Clone)]
pub struct Components<'a> {
    source: &'a str,
}

impl<'a> Iterator for Components<'a> {
    type Item = Component<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        self.source = self.source.trim_start_matches(SEP);

        let slice = match self.source.find(SEP) {
            Some(i) => {
                let (slice, rest) = self.source.split_at(i);
                self.source = rest.trim_start_matches(SEP);
                slice
            }
            None => mem::take(&mut self.source),
        };

        match slice {
            "" => None,
            CURRENT_STR => Some(Component::CurDir),
            PARENT_STR => Some(Component::ParentDir),
            slice => Some(Component::Normal(slice)),
        }
    }
}

impl<'a> DoubleEndedIterator for Components<'a> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.source = self.source.trim_end_matches(SEP);

        let slice = match self.source.rfind(SEP) {
            Some(i) => {
                let (rest, slice) = self.source.split_at(i + 1);
                self.source = rest.trim_end_matches(SEP);
                slice
            }
            None => mem::take(&mut self.source),
        };

        match slice {
            "" => None,
            CURRENT_STR => Some(Component::CurDir),
            PARENT_STR => Some(Component::ParentDir),
            slice => Some(Component::Normal(slice)),
        }
    }
}

impl<'a> Components<'a> {
    /// Construct a new component from the given string.
    fn new(source: &'a str) -> Components<'a> {
        Self { source }
    }

    /// Extracts a slice corresponding to the portion of the path remaining for
    /// iteration.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let mut components = Path::new("tmp/foo/bar.txt").components();
    /// components.next();
    /// components.next();
    ///
    /// assert_eq!("bar.txt", components.as_path());
    /// ```
    pub fn as_path(&self) -> &'a Path {
        Path::new(self.source)
    }
}

impl<'a> cmp::PartialEq for Components<'a> {
    fn eq(&self, other: &Components<'a>) -> bool {
        Iterator::eq(self.clone(), other.clone())
    }
}

/// An iterator over the [`Component`]s of a [`Path`], as
/// [`str`][std::str] slices.
///
/// This `struct` is created by the [`iter`][Path::iter] method.
#[derive(Clone)]
pub struct Iter<'a> {
    inner: Components<'a>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        self.inner.next().map(Component::as_str)
    }
}

impl<'a> DoubleEndedIterator for Iter<'a> {
    fn next_back(&mut self) -> Option<&'a str> {
        self.inner.next_back().map(Component::as_str)
    }
}

/// An owned, mutable path.
///
/// This type provides methods to manipulate path objects.
#[derive(Clone)]
pub struct PathBuf {
    inner: String,
}

impl PathBuf {
    /// Create a new path buffer.
    pub fn new() -> PathBuf {
        PathBuf {
            inner: String::new(),
        }
    }

    /// Internal constructor to allocate a path buf with the given
    /// capacity.
    fn with_capacity(cap: usize) -> PathBuf {
        PathBuf {
            inner: String::with_capacity(cap),
        }
    }

    /// Extends `self` with `path`.
    ///
    /// If `path` is absolute, it replaces the current path.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{PathBuf, Path};
    ///
    /// let mut path = PathBuf::new();
    /// path.push("foo");
    /// path.push("bar");
    ///
    /// assert_eq!("foo/bar", path);
    /// ```
    pub fn push<P: AsRef<Path>>(&mut self, path: P) {
        let other = path.as_ref();

        let other = if other.starts_with_sep() {
            &other.inner[1..]
        } else {
            &other.inner[..]
        };

        if !self.inner.is_empty() && !self.ends_with_sep() {
            self.inner.push(SEP);
        }

        self.inner.push_str(other)
    }

    /// Updates [`file_name`] to `file_name`.
    ///
    /// If [`file_name`] was [`None`], this is equivalent to pushing
    /// `file_name`.
    ///
    /// Otherwise it is equivalent to calling [`pop`] and then pushing
    /// `file_name`. The new path will be a sibling of the original path. (That
    /// is, it will have the same parent.)
    ///
    /// [`file_name`]: Path::file_name
    /// [`pop`]: PathBuf::pop
    /// [`None`]: https://doc.rust-lang.org/std/option/enum.Option.html
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::PathBuf;
    ///
    /// let mut buf = PathBuf::from("");
    /// assert!(buf.file_name() == None);
    /// buf.set_file_name("bar");
    /// assert_eq!(PathBuf::from("bar"), buf);
    ///
    /// assert!(buf.file_name().is_some());
    /// buf.set_file_name("baz.txt");
    /// assert_eq!(PathBuf::from("baz.txt"), buf);
    ///
    /// buf.push("bar");
    /// assert!(buf.file_name().is_some());
    /// buf.set_file_name("bar.txt");
    /// assert_eq!(PathBuf::from("baz.txt/bar.txt"), buf);
    /// ```
    pub fn set_file_name<S: AsRef<str>>(&mut self, file_name: S) {
        if self.file_name().is_some() {
            let popped = self.pop();
            debug_assert!(popped);
        }

        self.push(file_name.as_ref());
    }

    /// Updates [`extension`] to `extension`.
    ///
    /// Returns `false` and does nothing if
    /// [`file_name`][Path::file_name] is [`None`], returns `true` and
    /// updates the extension otherwise.
    ///
    /// If [`extension`] is [`None`], the extension is added; otherwise it is
    /// replaced.
    ///
    /// [`extension`]: Path::extension
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Path, PathBuf};
    ///
    /// let mut p = PathBuf::from("feel/the");
    ///
    /// p.set_extension("force");
    /// assert_eq!(Path::new("feel/the.force"), p);
    ///
    /// p.set_extension("dark_side");
    /// assert_eq!(Path::new("feel/the.dark_side"), p);
    ///
    /// assert!(p.pop());
    /// p.set_extension("nothing");
    /// assert_eq!(Path::new("feel.nothing"), p);
    /// ```
    pub fn set_extension<S: AsRef<str>>(&mut self, extension: S) -> bool {
        let file_stem = match self.file_stem() {
            Some(stem) => stem,
            None => return false,
        };

        let end_file_stem = file_stem[file_stem.len()..].as_ptr() as usize;
        let start = self.inner.as_ptr() as usize;
        self.inner.truncate(end_file_stem.wrapping_sub(start));

        let extension = extension.as_ref();

        if !extension.is_empty() {
            self.inner.push(STEM_SEP);
            self.inner.push_str(extension);
        }

        true
    }

    /// Truncates `self` to [`parent`][Path::parent].
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Path, PathBuf};
    ///
    /// let mut p = PathBuf::from("test/test.rs");
    ///
    /// assert_eq!(true, p.pop());
    /// assert_eq!(Path::new("test"), p);
    /// assert_eq!(true, p.pop());
    /// assert_eq!(Path::new(""), p);
    /// assert_eq!(false, p.pop());
    /// assert_eq!(Path::new(""), p);
    /// ```
    pub fn pop(&mut self) -> bool {
        match self.parent().map(|p| p.inner.len()) {
            Some(len) => {
                self.inner.truncate(len);
                true
            }
            None => false,
        }
    }

    /// Coerce to a [`Path`] slice.
    pub fn as_path(&self) -> &Path {
        self
    }

    /// Consumes the `PathBuf`, yielding its internal [`String`]
    /// storage.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::PathBuf;
    ///
    /// let p = PathBuf::from("/the/head");
    /// let string = p.into_string();
    /// assert_eq!(string, "/the/head".to_owned());
    /// ```
    pub fn into_string(self) -> String {
        self.inner
    }

    /// Converts this `PathBuf` into a [boxed][std::boxed::Box]
    /// [`Path`].
    pub fn into_boxed_relative_path(self) -> Box<Path> {
        let rw = Box::into_raw(self.inner.into_boxed_str()) as *mut Path;
        unsafe { Box::from_raw(rw) }
    }
}

impl Default for PathBuf {
    fn default() -> Self {
        PathBuf::new()
    }
}

impl<'a> From<&'a Path> for Cow<'a, Path> {
    #[inline]
    fn from(s: &'a Path) -> Cow<'a, Path> {
        Cow::Borrowed(s)
    }
}

impl<'a> From<PathBuf> for Cow<'a, Path> {
    #[inline]
    fn from(s: PathBuf) -> Cow<'a, Path> {
        Cow::Owned(s)
    }
}

impl fmt::Debug for PathBuf {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{:?}", &self.inner)
    }
}

impl AsRef<Path> for PathBuf {
    fn as_ref(&self) -> &Path {
        Path::new(&self.inner)
    }
}

impl AsRef<str> for Path {
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

impl Borrow<Path> for PathBuf {
    fn borrow(&self) -> &Path {
        self.deref()
    }
}

impl<'a, T: ?Sized + AsRef<str>> From<&'a T> for PathBuf {
    fn from(path: &'a T) -> PathBuf {
        PathBuf {
            inner: path.as_ref().to_owned(),
        }
    }
}

impl From<String> for PathBuf {
    fn from(path: String) -> PathBuf {
        PathBuf { inner: path }
    }
}

impl From<PathBuf> for String {
    fn from(path: PathBuf) -> String {
        path.into_string()
    }
}

impl ops::Deref for PathBuf {
    type Target = Path;

    fn deref(&self) -> &Path {
        Path::new(&self.inner)
    }
}

impl cmp::PartialEq for PathBuf {
    fn eq(&self, other: &PathBuf) -> bool {
        self.components() == other.components()
    }
}

impl cmp::Eq for PathBuf {}

impl cmp::PartialOrd for PathBuf {
    fn partial_cmp(&self, other: &PathBuf) -> Option<cmp::Ordering> {
        self.components().partial_cmp(other.components())
    }
}

impl cmp::Ord for PathBuf {
    fn cmp(&self, other: &PathBuf) -> cmp::Ordering {
        self.components().cmp(other.components())
    }
}

impl Hash for PathBuf {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.as_path().hash(h)
    }
}

impl<P: AsRef<Path>> Extend<P> for PathBuf {
    #[inline]
    fn extend<I: IntoIterator<Item = P>>(&mut self, iter: I) {
        iter.into_iter().for_each(move |p| self.push(p.as_ref()));
    }
}

impl<P: AsRef<Path>> FromIterator<P> for PathBuf {
    #[inline]
    fn from_iter<I: IntoIterator<Item = P>>(iter: I) -> PathBuf {
        let mut buf = PathBuf::new();
        buf.extend(iter);
        buf
    }
}

/// A borrowed, immutable path.
#[repr(transparent)]
pub struct Path {
    inner: str,
}

/// An error returned from [strip_prefix] if the prefix was not found.
///
/// [strip_prefix]: Path::strip_prefix
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StripPrefixError(());

impl Path {
    /// Directly wraps a string slice as a `Path` slice.
    pub fn new<S: AsRef<str> + ?Sized>(s: &S) -> &Path {
        unsafe { &*(s.as_ref() as *const str as *const Path) }
    }

    /// Yields the underlying [`str`][std::str] slice.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(Path::new("foo.txt").as_str(), "foo.txt");
    /// ```
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// Creates an owned [`PathBuf`] with path adjoined to self.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let path = Path::new("foo/bar");
    /// assert_eq!("foo/bar/baz", path.join("baz"));
    /// ```
    pub fn join<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let mut out = self.to_path_buf();
        out.push(path);
        out
    }

    /// Iterate over all components in this path.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Component, Path};
    ///
    /// let path = Path::new("foo/bar/baz");
    /// let mut it = path.components();
    ///
    /// assert_eq!(Some(Component::Normal("foo")), it.next());
    /// assert_eq!(Some(Component::Normal("bar")), it.next());
    /// assert_eq!(Some(Component::Normal("baz")), it.next());
    /// assert_eq!(None, it.next());
    /// ```
    pub fn components(&self) -> Components {
        Components::new(&self.inner)
    }

    /// Produces an iterator over the path's components viewed as
    /// [`str`][std::str] slices.
    ///
    /// For more information about the particulars of how the path is separated
    /// into components, see [`components`][Self::components].
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let mut it = Path::new("/tmp/foo.txt").iter();
    /// assert_eq!(it.next(), Some("tmp"));
    /// assert_eq!(it.next(), Some("foo.txt"));
    /// assert_eq!(it.next(), None)
    /// ```
    pub fn iter(&self) -> Iter {
        Iter {
            inner: self.components(),
        }
    }

    /// Convert to an owned [`PathBuf`].
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(self.inner.to_owned())
    }

    /// Returns a path, without its final [`Component`] if there is
    /// one.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(Some(Path::new("foo")), Path::new("foo/bar").parent());
    /// assert_eq!(Some(Path::new("")), Path::new("foo").parent());
    /// assert_eq!(None, Path::new("").parent());
    /// ```
    pub fn parent(&self) -> Option<&Path> {
        use self::Component::*;

        if self.inner.is_empty() {
            return None;
        }

        let mut it = self.components();
        while let Some(CurDir) = it.next_back() {}
        Some(it.as_path())
    }

    /// Returns the final component of the `Path`, if there is one.
    ///
    /// If the path is a normal file, this is the file name. If it's the path of
    /// a directory, this is the directory name.
    ///
    /// Returns [`None`] If the path terminates in `..`.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(Some("bin"), Path::new("usr/bin/").file_name());
    /// assert_eq!(Some("foo.txt"), Path::new("tmp/foo.txt").file_name());
    /// assert_eq!(Some("foo.txt"), Path::new("tmp/foo.txt/").file_name());
    /// assert_eq!(Some("foo.txt"), Path::new("foo.txt/.").file_name());
    /// assert_eq!(Some("foo.txt"), Path::new("foo.txt/.//").file_name());
    /// assert_eq!(None, Path::new("foo.txt/..").file_name());
    /// assert_eq!(None, Path::new("/").file_name());
    /// ```
    pub fn file_name(&self) -> Option<&str> {
        use self::Component::*;

        let mut it = self.components();

        while let Some(c) = it.next_back() {
            return match c {
                CurDir => continue,
                Normal(name) => Some(name),
                _ => None,
            };
        }

        None
    }

    /// Returns a path that, when joined onto `base`, yields `self`.
    ///
    /// # Errors
    ///
    /// If `base` is not a prefix of `self` (i.e.
    /// [starts_with][Self::starts_with] returns `false`), returns [`Err`].
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let path = Path::new("test/haha/foo.txt");
    ///
    /// assert_eq!(path.strip_prefix("test"), Ok(Path::new("haha/foo.txt")));
    /// assert_eq!(path.strip_prefix("test").is_ok(), true);
    /// assert_eq!(path.strip_prefix("haha").is_ok(), false);
    /// ```
    pub fn strip_prefix<P: AsRef<Path>>(&self, base: P) -> Result<&Path, StripPrefixError> {
        iter_after(self.components(), base.as_ref().components())
            .map(|c| c.as_path())
            .ok_or(StripPrefixError(()))
    }

    /// Determines whether `base` is a prefix of `self`.
    ///
    /// Only considers whole path components to match.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let path = Path::new("etc/passwd");
    ///
    /// assert!(path.starts_with("etc"));
    ///
    /// assert!(!path.starts_with("e"));
    /// ```
    pub fn starts_with<P: AsRef<Path>>(&self, base: P) -> bool {
        iter_after(self.components(), base.as_ref().components()).is_some()
    }

    /// Determines whether `child` is a suffix of `self`.
    ///
    /// Only considers whole path components to match.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let path = Path::new("etc/passwd");
    ///
    /// assert!(path.ends_with("passwd"));
    /// ```
    pub fn ends_with<P: AsRef<Path>>(&self, child: P) -> bool {
        iter_after(self.components().rev(), child.as_ref().components().rev()).is_some()
    }

    /// Determines whether `self` is normalized.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// // These are normalized.
    /// assert!(Path::new("").is_normalized());
    /// assert!(Path::new("baz.txt").is_normalized());
    /// assert!(Path::new("foo/bar/baz.txt").is_normalized());
    /// assert!(Path::new("..").is_normalized());
    /// assert!(Path::new("../..").is_normalized());
    /// assert!(Path::new("../../foo/bar/baz.txt").is_normalized());
    ///
    /// // These are not normalized.
    /// assert!(!Path::new(".").is_normalized());
    /// assert!(!Path::new("./baz.txt").is_normalized());
    /// assert!(!Path::new("foo/..").is_normalized());
    /// assert!(!Path::new("foo/../baz.txt").is_normalized());
    /// assert!(!Path::new("foo/.").is_normalized());
    /// assert!(!Path::new("foo/./baz.txt").is_normalized());
    /// assert!(!Path::new("../foo/./bar/../baz.txt").is_normalized());
    /// ```
    pub fn is_normalized(&self) -> bool {
        self.components()
            .skip_while(|c| matches!(c, Component::ParentDir))
            .all(|c| matches!(c, Component::Normal(_)))
    }

    /// Creates an owned [`PathBuf`] like `self` but with the given file
    /// name.
    ///
    /// See [set_file_name][PathBuf::set_file_name] for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Path, PathBuf};
    ///
    /// let path = Path::new("tmp/foo.txt");
    /// assert_eq!(path.with_file_name("bar.txt"), PathBuf::from("tmp/bar.txt"));
    ///
    /// let path = Path::new("tmp");
    /// assert_eq!(path.with_file_name("var"), PathBuf::from("var"));
    /// ```
    pub fn with_file_name<S: AsRef<str>>(&self, file_name: S) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.set_file_name(file_name);
        buf
    }

    /// Extracts the stem (non-extension) portion of
    /// [`file_name`][Self::file_name].
    ///
    /// The stem is:
    ///
    /// * [`None`], if there is no file name;
    /// * The entire file name if there is no embedded `.`;
    /// * The entire file name if the file name begins with `.` and has no other
    ///   `.`s within;
    /// * Otherwise, the portion of the file name before the final `.`
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let path = Path::new("foo.rs");
    ///
    /// assert_eq!("foo", path.file_stem().unwrap());
    /// ```
    pub fn file_stem(&self) -> Option<&str> {
        self.file_name()
            .map(split_file_at_dot)
            .and_then(|(before, after)| before.or(after))
    }

    /// Extracts the extension of [`file_name`][Self::file_name], if possible.
    ///
    /// The extension is:
    ///
    /// * [`None`], if there is no file name;
    /// * [`None`], if there is no embedded `.`;
    /// * [`None`], if the file name begins with `.` and has no other `.`s
    ///   within;
    /// * Otherwise, the portion of the file name after the final `.`
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(Some("rs"), Path::new("foo.rs").extension());
    /// assert_eq!(None, Path::new(".rs").extension());
    /// assert_eq!(Some("rs"), Path::new("foo.rs/.").extension());
    /// ```
    pub fn extension(&self) -> Option<&str> {
        self.file_name()
            .map(split_file_at_dot)
            .and_then(|(before, after)| before.and(after))
    }

    /// Creates an owned [`PathBuf`] like `self` but with the given
    /// extension.
    ///
    /// See [set_extension][PathBuf::set_extension] for more details.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::{Path, PathBuf};
    ///
    /// let path = Path::new("foo.rs");
    /// assert_eq!(path.with_extension("txt"), PathBuf::from("foo.txt"));
    /// ```
    pub fn with_extension<S: AsRef<str>>(&self, extension: S) -> PathBuf {
        let mut buf = self.to_path_buf();
        buf.set_extension(extension);
        buf
    }

    /// Build an owned [`PathBuf`], joined with the given path and
    /// normalized.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(
    ///     Path::new("foo/baz.txt"),
    ///     Path::new("foo/bar").join_normalized("../baz.txt").as_path()
    /// );
    ///
    /// assert_eq!(
    ///     Path::new("../foo/baz.txt"),
    ///     Path::new("../foo/bar").join_normalized("../baz.txt").as_path()
    /// );
    /// ```
    pub fn join_normalized<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let mut buf = PathBuf::new();
        relative_traversal(&mut buf, self.components());
        relative_traversal(&mut buf, path.as_ref().components());
        buf
    }

    /// Return an owned [`PathBuf`], with all non-normal components
    /// moved to the beginning of the path.
    ///
    /// This permits for a normalized representation of different relative
    /// components.
    ///
    /// Normalization is a _destructive_ operation if the path references an
    /// actual filesystem path. An example of this is symlinks under unix, a
    /// path like `foo/../bar` might reference a different location other than
    /// `./bar`.
    ///
    /// Normalization is a logical operation that is only valid if the path is
    /// part of some context which doesn't have semantics that causes it
    /// to break, like symbolic links.
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(
    ///     "../foo/baz.txt",
    ///     Path::new("../foo/./bar/../baz.txt").normalize()
    /// );
    ///
    /// assert_eq!(
    ///     "",
    ///     Path::new(".").normalize()
    /// );
    /// ```
    pub fn normalize(&self) -> PathBuf {
        let mut buf = PathBuf::with_capacity(self.inner.len());
        relative_traversal(&mut buf, self.components());
        buf
    }

    /// Constructs a path from the current path, to `path`.
    ///
    /// This function will return the empty [`Path`] `""` if this source
    /// contains unnamed components like `..` that would have to be traversed to
    /// reach the destination `path`. This is necessary since we have no way of
    /// knowing what the names of those components are when we're building the
    /// new path.
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// // Here we don't know what directories `../..` refers to, so there's no
    /// // way to construct a path back to `bar` in the current directory from
    /// // `../..`.
    /// let from = Path::new("../../foo/relative-path");
    /// let to = Path::new("bar");
    /// assert_eq!("", from.relative(to));
    /// ```
    ///
    /// One exception to this is when two paths contains a common prefix at
    /// which point there's no need to know what the names of those unnamed
    /// components are.
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// let from = Path::new("../../foo/bar");
    /// let to = Path::new("../../foo/baz");
    ///
    /// assert_eq!("../baz", from.relative(to));
    ///
    /// let from = Path::new("../a/../../foo/bar");
    /// let to = Path::new("../../foo/baz");
    ///
    /// assert_eq!("../baz", from.relative(to));
    /// ```
    ///
    /// # Examples
    ///
    /// ```
    /// use umifs::path::Path;
    ///
    /// assert_eq!(
    ///     "../../e/f",
    ///     Path::new("a/b/c/d").relative(Path::new("a/b/e/f"))
    /// );
    ///
    /// assert_eq!(
    ///     "../bbb",
    ///     Path::new("a/../aaa").relative(Path::new("b/../bbb"))
    /// );
    ///
    /// let a = Path::new("git/relative-path");
    /// let b = Path::new("git");
    /// assert_eq!("relative-path", b.relative(a));
    /// assert_eq!("..", a.relative(b));
    ///
    /// let a = Path::new("foo/bar/bap/foo.h");
    /// let b = Path::new("../arch/foo.h");
    /// assert_eq!("../../../../../arch/foo.h", a.relative(b));
    /// assert_eq!("", b.relative(a));
    /// ```
    pub fn relative<P: AsRef<Path>>(&self, path: P) -> PathBuf {
        let mut from = PathBuf::with_capacity(self.inner.len());
        let mut to = PathBuf::with_capacity(path.as_ref().inner.len());

        relative_traversal(&mut from, self.components());
        relative_traversal(&mut to, path.as_ref().components());

        let mut it_from = from.components();
        let mut it_to = to.components();

        // Strip a common prefixes - if any.
        let (lead_from, lead_to) = loop {
            match (it_from.next(), it_to.next()) {
                (Some(f), Some(t)) if f == t => continue,
                (f, t) => {
                    break (f, t);
                }
            }
        };

        // Special case: The path we are traversing from can't contain unnamed
        // components. A path might be any path, like `/`, or
        // `/foo/bar/baz`, and these components cannot be named in the relative
        // traversal.
        //
        // Also note that `relative_traversal` guarantees that all ParentDir
        // components are at the head of the path being built.
        if lead_from == Some(Component::ParentDir) {
            return PathBuf::new();
        }

        let head = lead_from.into_iter().chain(it_from);
        let tail = lead_to.into_iter().chain(it_to);

        let mut buf = PathBuf::with_capacity(usize::max(from.inner.len(), to.inner.len()));

        for c in head.map(|_| Component::ParentDir).chain(tail) {
            buf.push(c.as_str());
        }

        buf
    }

    /// Check if path starts with a path separator.
    #[inline]
    fn starts_with_sep(&self) -> bool {
        self.inner.starts_with(SEP)
    }

    /// Check if path ends with a path separator.
    #[inline]
    fn ends_with_sep(&self) -> bool {
        self.inner.ends_with(SEP)
    }
}

/// Conversion from a [Box<str>] reference to a [Box<Path>].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path: Box<Path> = Box::<str>::from("foo/bar").into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<Box<str>> for Box<Path> {
    #[inline]
    fn from(boxed: Box<str>) -> Box<Path> {
        let rw = Box::into_raw(boxed) as *mut Path;
        unsafe { Box::from_raw(rw) }
    }
}

/// Conversion from a [str] reference to a [Box<Path>].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path: Box<Path> = "foo/bar".into();
/// assert_eq!(&*path, "foo/bar");
///
/// let path: Box<Path> = Path::new("foo/bar").into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl<T> From<&T> for Box<Path>
where
    T: ?Sized + AsRef<str>,
{
    #[inline]
    fn from(path: &T) -> Box<Path> {
        Box::<Path>::from(Box::<str>::from(path.as_ref()))
    }
}

/// Conversion from [PathBuf] to [Box<Path>].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use umifs::path::{Path, PathBuf};
///
/// let path = PathBuf::from("foo/bar");
/// let path: Box<Path> = path.into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<PathBuf> for Box<Path> {
    #[inline]
    fn from(path: PathBuf) -> Box<Path> {
        let boxed: Box<str> = path.inner.into();
        let rw = Box::into_raw(boxed) as *mut Path;
        unsafe { Box::from_raw(rw) }
    }
}

/// Clone implementation for [Box<Path>].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path: Box<Path> = Path::new("foo/bar").into();
/// let path2 = path.clone();
/// assert_eq!(&*path, &*path2);
/// ```
impl Clone for Box<Path> {
    #[inline]
    fn clone(&self) -> Self {
        self.to_path_buf().into_boxed_relative_path()
    }
}

/// Conversion from [Path] to [Arc<Path>].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use umifs::path::Path;
///
/// let path: Arc<Path> = Path::new("foo/bar").into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<&Path> for Arc<Path> {
    #[inline]
    fn from(path: &Path) -> Arc<Path> {
        let arc: Arc<str> = path.inner.into();
        let rw = Arc::into_raw(arc) as *const Path;
        unsafe { Arc::from_raw(rw) }
    }
}

/// Conversion from [PathBuf] to [Arc<Path>].
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use umifs::path::{Path, PathBuf};
///
/// let path = PathBuf::from("foo/bar");
/// let path: Arc<Path> = path.into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<PathBuf> for Arc<Path> {
    #[inline]
    fn from(path: PathBuf) -> Arc<Path> {
        let arc: Arc<str> = path.inner.into();
        let rw = Arc::into_raw(arc) as *const Path;
        unsafe { Arc::from_raw(rw) }
    }
}

/// Conversion from [PathBuf] to [Arc<Path>].
///
/// # Examples
///
/// ```
/// use std::rc::Rc;
/// use umifs::path::Path;
///
/// let path: Rc<Path> = Path::new("foo/bar").into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<&Path> for Rc<Path> {
    #[inline]
    fn from(path: &Path) -> Rc<Path> {
        let rc: Rc<str> = path.inner.into();
        let rw = Rc::into_raw(rc) as *const Path;
        unsafe { Rc::from_raw(rw) }
    }
}

/// Conversion from [PathBuf] to [Rc<Path>].
///
/// # Examples
///
/// ```
/// use std::rc::Rc;
/// use umifs::path::{Path, PathBuf};
///
/// let path = PathBuf::from("foo/bar");
/// let path: Rc<Path> = path.into();
/// assert_eq!(&*path, "foo/bar");
/// ```
impl From<PathBuf> for Rc<Path> {
    #[inline]
    fn from(path: PathBuf) -> Rc<Path> {
        let rc: Rc<str> = path.inner.into();
        let rw = Rc::into_raw(rc) as *const Path;
        unsafe { Rc::from_raw(rw) }
    }
}

/// [ToOwned] implementation for [Path].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path = Path::new("foo/bar").to_owned();
/// assert_eq!(path, "foo/bar");
/// ```
impl ToOwned for Path {
    type Owned = PathBuf;

    #[inline]
    fn to_owned(&self) -> PathBuf {
        self.to_path_buf()
    }
}

impl fmt::Debug for Path {
    #[inline]
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        write!(fmt, "{:?}", &self.inner)
    }
}

/// [AsRef<str>] implementation for [PathBuf].
///
/// # Examples
///
/// ```
/// use umifs::path::PathBuf;
///
/// let path = PathBuf::from("foo/bar");
/// let string: &str = path.as_ref();
/// assert_eq!(string, "foo/bar");
/// ```
impl AsRef<str> for PathBuf {
    #[inline]
    fn as_ref(&self) -> &str {
        &self.inner
    }
}

/// [AsRef<Path>] implementation for [String].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path: String = format!("foo/bar");
/// let path: &Path = path.as_ref();
/// assert_eq!(path, "foo/bar");
/// ```
impl AsRef<Path> for String {
    #[inline]
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}

/// [AsRef<Path>] implementation for [str].
///
/// # Examples
///
/// ```
/// use umifs::path::Path;
///
/// let path: &Path = "foo/bar".as_ref();
/// assert_eq!(path, Path::new("foo/bar"));
/// ```
impl AsRef<Path> for str {
    #[inline]
    fn as_ref(&self) -> &Path {
        Path::new(self)
    }
}

impl AsRef<Path> for Path {
    #[inline]
    fn as_ref(&self) -> &Path {
        self
    }
}

impl cmp::PartialEq for Path {
    #[inline]
    fn eq(&self, other: &Path) -> bool {
        self.components() == other.components()
    }
}

impl cmp::Eq for Path {}

impl cmp::PartialOrd for Path {
    #[inline]
    fn partial_cmp(&self, other: &Path) -> Option<cmp::Ordering> {
        self.components().partial_cmp(other.components())
    }
}

impl cmp::Ord for Path {
    #[inline]
    fn cmp(&self, other: &Path) -> cmp::Ordering {
        self.components().cmp(other.components())
    }
}

impl Hash for Path {
    #[inline]
    fn hash<H: Hasher>(&self, h: &mut H) {
        for c in self.components() {
            c.hash(h);
        }
    }
}

impl fmt::Display for Path {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl fmt::Display for PathBuf {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

macro_rules! impl_cmp {
    ($lhs:ty, $rhs:ty) => {
        impl<'a, 'b> PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                <Path as PartialEq>::eq(self, other)
            }
        }

        impl<'a, 'b> PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                <Path as PartialEq>::eq(self, other)
            }
        }

        impl<'a, 'b> PartialOrd<$rhs> for $lhs {
            #[inline]
            fn partial_cmp(&self, other: &$rhs) -> Option<cmp::Ordering> {
                <Path as PartialOrd>::partial_cmp(self, other)
            }
        }

        impl<'a, 'b> PartialOrd<$lhs> for $rhs {
            #[inline]
            fn partial_cmp(&self, other: &$lhs) -> Option<cmp::Ordering> {
                <Path as PartialOrd>::partial_cmp(self, other)
            }
        }
    };
}

impl_cmp!(PathBuf, Path);
impl_cmp!(PathBuf, &'a Path);
impl_cmp!(Cow<'a, Path>, Path);
impl_cmp!(Cow<'a, Path>, &'b Path);
impl_cmp!(Cow<'a, Path>, PathBuf);

macro_rules! impl_cmp_str {
    ($lhs:ty, $rhs:ty) => {
        impl<'a, 'b> PartialEq<$rhs> for $lhs {
            #[inline]
            fn eq(&self, other: &$rhs) -> bool {
                <Path as PartialEq>::eq(self, other.as_ref())
            }
        }

        impl<'a, 'b> PartialEq<$lhs> for $rhs {
            #[inline]
            fn eq(&self, other: &$lhs) -> bool {
                <Path as PartialEq>::eq(self.as_ref(), other)
            }
        }

        impl<'a, 'b> PartialOrd<$rhs> for $lhs {
            #[inline]
            fn partial_cmp(&self, other: &$rhs) -> Option<cmp::Ordering> {
                <Path as PartialOrd>::partial_cmp(self, other.as_ref())
            }
        }

        impl<'a, 'b> PartialOrd<$lhs> for $rhs {
            #[inline]
            fn partial_cmp(&self, other: &$lhs) -> Option<cmp::Ordering> {
                <Path as PartialOrd>::partial_cmp(self.as_ref(), other)
            }
        }
    };
}

impl_cmp_str!(PathBuf, str);
impl_cmp_str!(PathBuf, &'a str);
impl_cmp_str!(PathBuf, String);
impl_cmp_str!(Path, str);
impl_cmp_str!(Path, &'a str);
impl_cmp_str!(Path, String);
impl_cmp_str!(&'a Path, str);
impl_cmp_str!(&'a Path, String);
