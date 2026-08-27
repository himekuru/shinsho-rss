use anyhow::{anyhow, Context, Result};
use chrono::NaiveDate;
use regex::Regex;
use reqwest::Client;
use rss::{ChannelBuilder, GuidBuilder, ItemBuilder};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use url::Url;

const STATE_PATH: &str = "state.json";
const RSS_PATH: &str = "shinsho.xml";
const MAX_FEED_ITEMS: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Book {
    id: String,
    label: String,
    title: String,
    author: String,
    publication_date: Option<NaiveDate>,
    isbn: Option<String>,
    url: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    books: BTreeMap<String, Book>,
}

#[derive(Debug, Serialize)]
struct IwanamiQuery {
    keyword: String,
    label: u8,
    genre: Option<u32>,
    genre_or: Option<u32>,
    offset: u32,
    limit: u32,
    random_seed: Option<u32>,
    order: String,
    direction: String,
    first: Option<String>,
    id: Option<String>,
    filter_field: Option<String>,
    filter_start: u32,
    filter_end: u32,
    filter_stock: bool,
    filter_electric: bool,
    filter_bunko100: bool,
}

#[derive(Debug, Deserialize)]
struct IwanamiSearchResponse {
    #[serde(default)]
    books: Vec<IwanamiBook>,
}

#[derive(Debug, Deserialize)]
struct IwanamiBook {
    id: serde_json::Value,
    #[serde(default)]
    title1: String,
    #[serde(default)]
    title2: String,
    #[serde(default)]
    subtitle: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    date: Option<u32>,
    #[serde(default)]
    isbn: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::builder()
        .user_agent("shinsho-rss/0.1 (+personal RSS reader)")
        .build()?;

    let mut state = load_state(STATE_PATH).await?;
    let mut fetched = Vec::new();
    let mut failures = 0usize;

    macro_rules! collect_source {
        ($name:expr, $future:expr) => {
            match ($future).await {
                Ok(books) => fetched.extend(books),
                Err(err) => {
                    failures += 1;
                    eprintln!("source failed: {}: {:#}", $name, err);
                }
            }
        };
    }

    collect_source!("岩波新書", fetch_iwanami(&client, 2, "岩波新書"));
    collect_source!("岩波ジュニア新書", fetch_iwanami(&client, 4, "岩波ジュニア新書"));
    collect_source!("中公新書", fetch_chuko(&client, &state));
    collect_source!(
        "講談社現代新書",
        fetch_kodansha(
            &client,
            "https://www.kodansha.co.jp/book/labels/gendai-shinsho",
            "講談社現代新書",
            &state,
        )
    );
    collect_source!(
        "ブルーバックス",
        fetch_kodansha(
            &client,
            "https://www.kodansha.co.jp/book/labels/bluebacks",
            "ブルーバックス",
            &state,
        )
    );
    collect_source!(
        "ちくま新書",
        fetch_chikuma(
            &client,
            "https://www.chikumashobo.co.jp/chikuma_shinsho/",
            "ちくま新書",
        )
    );
    collect_source!(
        "ちくま学芸文庫",
        fetch_chikuma(
            &client,
            "https://www.chikumashobo.co.jp/chikuma_gakugei_bunko/",
            "ちくま学芸文庫",
        )
    );
    collect_source!("NHK出版新書", fetch_nhk(&client));

    if failures == 8 {
        return Err(anyhow!("all 8 sources failed"));
    }

    let before = state.books.len();
    for book in fetched {
        state.books.insert(book.id.clone(), book);
    }
    let added = state.books.len().saturating_sub(before);

    save_state(STATE_PATH, &state).await?;
    let feed_link = std::env::var("FEED_LINK")
        .unwrap_or_else(|_| "https://example.invalid/shinsho.xml".to_string());
    write_rss(RSS_PATH, &state, &feed_link).await?;

    println!("{} books stored ({} newly added); wrote {}", state.books.len(), added, RSS_PATH);
    Ok(())
}

async fn fetch_iwanami(client: &Client, label_id: u8, label: &str) -> Result<Vec<Book>> {
    let query = IwanamiQuery {
        keyword: String::new(),
        label: label_id,
        genre: None,
        genre_or: None,
        offset: 0,
        limit: 48,
        random_seed: None,
        order: "date".into(),
        direction: "DESC".into(),
        first: None,
        id: None,
        filter_field: None,
        filter_start: 0,
        filter_end: 0,
        filter_stock: false,
        filter_electric: false,
        filter_bunko100: false,
    };

    let res = client
        .post("https://catalog.iwanami.co.jp/api/search")
        .json(&query)
        .send()
        .await?
        .error_for_status()?
        .json::<IwanamiSearchResponse>()
        .await?;

    Ok(res
        .books
        .into_iter()
        .map(|b| {
            let id = match b.id {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            let title = join_nonempty([b.title1.as_str(), b.title2.as_str(), b.subtitle.as_str()], " ");
            Book {
                id: format!("iwanami:{id}"),
                label: label.to_string(),
                title,
                author: normalize_space(&b.author),
                publication_date: b.date.and_then(parse_yyyymmdd),
                isbn: b.isbn.map(|s| normalize_isbn(&s)),
                url: format!("https://catalog.iwanami.co.jp/book/{id}"),
            }
        })
        .collect())
}

async fn fetch_chuko(client: &Client, state: &State) -> Result<Vec<Book>> {
    let base = "https://www.chuko.co.jp/shinsho/";
    let html = get_text(client, base).await?;
    let doc = Html::parse_document(&html);
    let row_sel = sel("#book_list #list > li.linkbox")?;
    let title_sel = sel(".book_txt h3")?;
    let subtitle_sel = sel(".book_txt .book_tit")?;
    let author_sel = sel(".book_txt .book_auth")?;
    let desc_sel = sel(".book_txt .book_desc")?;
    let link_sel = sel(".fullstory a[href]")?;
    let date_re = Regex::new(r"(?P<y>\d{4})/(?P<m>\d{1,2})/(?P<d>\d{1,2})\s*刊行")?;

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let url = match row.select(&link_sel).next().and_then(|a| a.value().attr("href")) {
            Some(v) => absolutize(base, v)?,
            None => continue,
        };
        let id = format!("chuko:{url}");
        let title = text_first(row, &title_sel).unwrap_or_default();
        let subtitle = text_first(row, &subtitle_sel).unwrap_or_default();
        let author = text_first(row, &author_sel).unwrap_or_default();
        let publication_date = row
            .select(&desc_sel)
            .filter_map(|e| date_re.captures(&element_text(e)))
            .find_map(|c| ymd_caps(&c));

        let isbn = if let Some(old) = state.books.get(&id).and_then(|b| b.isbn.clone()) {
            Some(old)
        } else {
            fetch_chuko_isbn(client, &url).await.ok().flatten()
        };

        out.push(Book {
            id,
            label: "中公新書".into(),
            title: join_nonempty([title.as_str(), subtitle.as_str()], "　"),
            author: normalize_space(&author),
            publication_date,
            isbn,
            url,
        });
    }
    Ok(out)
}

async fn fetch_chuko_isbn(client: &Client, url: &str) -> Result<Option<String>> {
    let html = get_text(client, url).await?;
    let doc = Html::parse_document(&html);
    let jsonld_sel = sel(r#"script[type="application/ld+json"]"#)?;
    for node in doc.select(&jsonld_sel) {
        let raw = node.text().collect::<String>();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) {
            if let Some(isbn) = find_json_isbn(&value) {
                return Ok(Some(normalize_isbn(isbn)));
            }
        }
    }
    Ok(None)
}

async fn fetch_kodansha(client: &Client, list_url: &str, label: &str, state: &State) -> Result<Vec<Book>> {
    let html = get_text(client, list_url).await?;
    let doc = Html::parse_document(&html);
    let row_sel = sel("a.app-product-list-item")?;
    let title_sel = sel(".app-product-list-item-detail-name")?;
    let author_sel = sel(".app-product-list-item-detail-authors")?;

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let href = match row.value().attr("href") {
            Some(v) => v,
            None => continue,
        };
        let url = absolutize(list_url, href)?;
        let id = format!("kodansha:{url}");
        let title = text_first(row, &title_sel).unwrap_or_default();
        let author = text_first(row, &author_sel).unwrap_or_default();

        let (isbn, date) = if let Some(old) = state.books.get(&id) {
            if old.isbn.is_some() && old.publication_date.is_some() {
                (old.isbn.clone(), old.publication_date)
            } else {
                fetch_kodansha_detail(client, &url).await.unwrap_or((old.isbn.clone(), old.publication_date))
            }
        } else {
            fetch_kodansha_detail(client, &url).await.unwrap_or((None, None))
        };

        out.push(Book {
            id,
            label: label.to_string(),
            title: normalize_space(&title),
            author: clean_author_prefix(&author),
            publication_date: date,
            isbn,
            url,
        });
    }
    Ok(out)
}

async fn fetch_kodansha_detail(client: &Client, url: &str) -> Result<(Option<String>, Option<NaiveDate>)> {
    let html = get_text(client, url).await?;
    let doc = Html::parse_document(&html);

    let isbn_sel = sel(r#"meta[property="books:isbn"]"#)?;
    let isbn = doc
        .select(&isbn_sel)
        .next()
        .and_then(|e| e.value().attr("content"))
        .map(normalize_isbn);

    let specs_sel = sel(r#"section[data-scroll-anchor="specs"]"#)?;
    let section_sel = sel("section")?;
    let h5_sel = sel("h5")?;
    let p_sel = sel("p")?;
    let date_re = Regex::new(r"(?P<y>\d{4})年(?P<m>\d{1,2})月(?P<d>\d{1,2})日")?;

    let mut date = None;
    if let Some(specs) = doc.select(&specs_sel).next() {
        // 紙版が先に置かれているので、最初の「発売日」行を採用する。
        for section in specs.select(&section_sel) {
            let key = text_first(section, &h5_sel).unwrap_or_default();
            if normalize_space(&key) == "発売日" {
                if let Some(value) = text_first(section, &p_sel) {
                    date = date_re.captures(&value).and_then(|c| ymd_caps(&c));
                    break;
                }
            }
        }
    }
    Ok((isbn, date))
}

async fn fetch_chikuma(client: &Client, list_url: &str, label: &str) -> Result<Vec<Book>> {
    let html = get_text(client, list_url).await?;
    let doc = Html::parse_document(&html);
    let row_sel = sel(r#"a[href^="/product/978"]"#)?;
    let title_sel = sel("h2")?;
    let subtitle_sel = sel("h4")?;
    let author_sel = sel("h3")?;
    let date_re = Regex::new(r"\b(?P<y>\d{4})/(?P<m>\d{2})/(?P<d>\d{2})\b")?;
    let isbn_re = Regex::new(r"978-\d-\d+-\d+-\d")?;
    let path_isbn_re = Regex::new(r"/product/(978\d{10})/")?;

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let href = match row.value().attr("href") {
            Some(v) => v,
            None => continue,
        };
        let url = absolutize(list_url, href)?;
        let whole = element_text(row);
        let title = text_first(row, &title_sel).unwrap_or_default();
        let subtitle = text_first(row, &subtitle_sel).unwrap_or_default();
        let author = row
            .select(&author_sel)
            .map(element_text)
            .filter(|s| !s.trim().is_empty())
            .collect::<Vec<_>>()
            .join("・");
        let publication_date = date_re.captures(&whole).and_then(|c| ymd_caps(&c));
        let isbn = isbn_re
            .find(&whole)
            .map(|m| normalize_isbn(m.as_str()))
            .or_else(|| path_isbn_re.captures(href).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()));

        out.push(Book {
            id: format!("chikuma:{}", isbn.clone().unwrap_or_else(|| url.clone())),
            label: label.to_string(),
            title: join_nonempty([title.as_str(), subtitle.as_str()], "　"),
            author: normalize_space(&author),
            publication_date,
            isbn,
            url,
        });
    }
    Ok(out)
}

async fn fetch_nhk(client: &Client) -> Result<Vec<Book>> {
    let list_url = "https://www.nhk-book.co.jp/list/midcategory-203.html";
    let html = get_text(client, list_url).await?;
    let doc = Html::parse_document(&html);
    let row_sel = sel(".itmlist-one")?;
    let series_sel = sel(".itmlist-one-series")?;
    let title_sel = sel(".itmlist-one-name")?;
    let author_sel = sel(".itmlist-one-persons")?;
    let date_sel = sel(".itmlist-one-about-cap")?;
    let link_sel = sel(r#".itmlist-one-blockDetail > a[href^="/detail/"]"#)?;
    let date_re = Regex::new(r"(?P<y>\d{4})年(?P<m>\d{1,2})月(?P<d>\d{1,2})日")?;
    let code_re = Regex::new(r"/detail/(\d+)\.html")?;

    let mut out = Vec::new();
    for row in doc.select(&row_sel) {
        let series = text_first(row, &series_sel).unwrap_or_default();
        if !normalize_space(&series).starts_with("ＮＨＫ出版新書") {
            continue;
        }
        let href = match row.select(&link_sel).next().and_then(|e| e.value().attr("href")) {
            Some(v) => v,
            None => continue,
        };
        let url = absolutize(list_url, href)?;
        let code = code_re
            .captures(href)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str())
            .unwrap_or(href);
        let title = text_first(row, &title_sel).unwrap_or_default();
        let author = text_first(row, &author_sel).unwrap_or_default();
        let date_text = text_first(row, &date_sel).unwrap_or_default();
        let publication_date = date_re.captures(&date_text).and_then(|c| ymd_caps(&c));

        out.push(Book {
            id: format!("nhk:{code}"),
            label: "NHK出版新書".into(),
            title: normalize_space(&title),
            author: clean_author_prefix(&author),
            publication_date,
            isbn: None,
            url,
        });
    }
    Ok(out)
}

async fn load_state(path: impl AsRef<Path>) -> Result<State> {
    match tokio::fs::read_to_string(path.as_ref()).await {
        Ok(s) => Ok(serde_json::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(e) => Err(e.into()),
    }
}

async fn save_state(path: impl AsRef<Path>, state: &State) -> Result<()> {
    let text = serde_json::to_string_pretty(state)?;
    tokio::fs::write(path, text).await?;
    Ok(())
}

async fn write_rss(path: impl AsRef<Path>, state: &State, feed_link: &str) -> Result<()> {
    let mut books: Vec<_> = state.books.values().cloned().collect();
    books.sort_by(|a, b| {
        b.publication_date
            .cmp(&a.publication_date)
            .then_with(|| b.label.cmp(&a.label))
            .then_with(|| b.title.cmp(&a.title))
    });

    let items = books
        .into_iter()
        .take(MAX_FEED_ITEMS)
        .map(|book| {
            let mut description = format!("{} / {}", book.label, book.author);
            if let Some(isbn) = &book.isbn {
                description.push_str(&format!(" / ISBN {isbn}"));
            }
            ItemBuilder::default()
                .title(Some(format!("[{}] {}", book.label, book.title)))
                .link(Some(book.url.clone()))
                .guid(Some(GuidBuilder::default().value(book.id.clone()).permalink(false).build()))
                .pub_date(book.publication_date.map(|d| format!("{} 00:00:00 +0900", d.format("%a, %d %b %Y"))))
                .description(Some(description))
                .build()
        })
        .collect::<Vec<_>>();

    let channel = ChannelBuilder::default()
        .title("新書・学術文庫 新刊")
        .link(feed_link)
        .description("岩波新書、岩波ジュニア新書、中公新書、講談社現代新書、ちくま新書、ブルーバックス、NHK出版新書、ちくま学芸文庫")
        .items(items)
        .build();

    tokio::fs::write(path, channel.to_string()).await?;
    Ok(())
}

async fn get_text(client: &Client, url: &str) -> Result<String> {
    Ok(client.get(url).send().await?.error_for_status()?.text().await?)
}

fn sel(s: &str) -> Result<Selector> {
    Selector::parse(s).map_err(|e| anyhow!("invalid selector {s:?}: {e:?}"))
}

fn element_text(e: ElementRef<'_>) -> String {
    normalize_space(&e.text().collect::<Vec<_>>().join(" "))
}

fn text_first(root: ElementRef<'_>, selector: &Selector) -> Option<String> {
    root.select(selector).next().map(element_text)
}

fn normalize_space(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clean_author_prefix(s: &str) -> String {
    let s = normalize_space(s);
    for prefix in ["著：", "[著]", "［著］"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return normalize_space(rest);
        }
    }
    s
}

fn normalize_isbn(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_digit() || *c == 'X').collect()
}

fn join_nonempty<const N: usize>(parts: [&str; N], sep: &str) -> String {
    parts
        .into_iter()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(sep)
}

fn absolutize(base: &str, href: &str) -> Result<String> {
    Ok(Url::parse(base)?.join(href)?.into())
}

fn ymd_caps(c: &regex::Captures<'_>) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        c.name("y")?.as_str().parse().ok()?,
        c.name("m")?.as_str().parse().ok()?,
        c.name("d")?.as_str().parse().ok()?,
    )
}

fn parse_yyyymmdd(v: u32) -> Option<NaiveDate> {
    let y = (v / 10000) as i32;
    let m = (v / 100 % 100) as u32;
    let d = (v % 100) as u32;
    NaiveDate::from_ymd_opt(y, m, d)
}

fn find_json_isbn(v: &serde_json::Value) -> Option<&str> {
    match v {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::String(s)) = map.get("isbn") {
                return Some(s);
            }
            map.values().find_map(find_json_isbn)
        }
        serde_json::Value::Array(a) => a.iter().find_map(find_json_isbn),
        _ => None,
    }
}
