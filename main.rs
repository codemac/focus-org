use chrono::prelude::*;
use lazy_static::*;
use regex::Regex;
use std::env;
use std::fs;
use std::io::prelude::*;
use std::io::BufReader;
use std::sync::mpsc;
use std::thread;

#[derive(Debug)]
struct Heading {
    title: String,
    level: usize,
    state: String,
    tags: Vec<String>,
    scheduled: Option<TimeRange>,
    deadline: Option<TimeRange>,
    logged: Vec<TimeRange>,
    logged_active: Option<NaiveDateTime>,
    timestamps: Vec<TimeRange>,
}

impl Heading {
    fn is_action(&self) -> bool {
        return self.state == "TODO"
            || self.state == "NEXT"
            || self.state == "STARTED"
            || self.state == "PROJECT";
    }

    // fn is_done(&self) -> bool {
    //     return self.state == "DONE" ||
    //         self.state == "NVM";
    // }

    fn is_clocked_now(&self) -> bool {
        if self.logged_active.is_none() {
            return false;
        }

        let now = Local::now().naive_local();
        return self.logged_active.unwrap() < now;
    }

    fn is_action_now(&self) -> bool {
        if !self.is_action() {
            return false;
        }

        let now = Local::now().naive_local();
        return match &self.scheduled {
            Some(s) => s.is_during(now),
            None => false,
        };
    }

    fn is_event_now(&self) -> bool {
        if self.is_action() {
            return false;
        }

        let now = Local::now().naive_local();
        return match &self.scheduled {
            Some(s) => s.is_during(now),
            None => false,
        };
        // loop over timestamps here!
    }

    fn is_overdue_now(&self) -> bool {
        if !self.is_action() {
            return false;
        }

        let now = Local::now().naive_local();
        return match &self.scheduled {
            Some(s) => s.is_before(now),
            None => match &self.deadline {
                Some(d) => d.is_before(now),
                None => false,
            },
        };
    }

    fn print_action(&self) -> String {
        return format!("[ ] {}", &self.title);
    }

    fn print_overdue(&self) -> String {
        return format!("^fg(orangered)[!]^fg() {}", &self.title);
    }

    fn print_clocked(&self) -> String {
        let now = Local::now().naive_local();
        let delta = now - self.logged_active.unwrap();
        return format!(
            "^fg(orange)[{:02}:{:02}:{:02}] {}^fg()",
            delta.num_hours(),
            delta.num_minutes(),
            delta.num_seconds(),
            &self.title
        );
    }

    fn print_event(&self) -> String {
        return format!("[ ] {}", &self.title);
    }

    fn most_recently_started(&self, now: NaiveDateTime) -> Option<&TimeRange> {
        if self.scheduled.is_some() {
            return self.scheduled.as_ref();
        }

        let mut recent_dur = chrono::Duration::max_value();
        let mut recent_range: Option<&TimeRange> = None;
        for tr in &self.timestamps {
            if now < tr.start {
                continue;
            }

            let start_dur = now - tr.start;
            if start_dur < recent_dur {
                recent_dur = start_dur;
                recent_range = Some(tr);
            }
        }
        return recent_range;
    }
}

#[derive(Debug)]
struct TimeRange {
    start: NaiveDateTime,
    end: NaiveDateTime,
}

impl TimeRange {
    fn is_during(&self, ts: NaiveDateTime) -> bool {
        return self.start < ts && ts < self.end;
    }

    fn is_before(&self, ts: NaiveDateTime) -> bool {
        return self.start < ts && self.end < ts;
    }

    // fn is_after(&self, ts: NaiveDateTime) -> bool{
    //     return ts < self.start  && ts < self.end;
    // }
}

fn most_recent<'a>(hs: &Vec<&'a Heading>) -> Option<&'a Heading> {
    if hs.len() == 0 {
        return None;
    }

    let now = Local::now().naive_local();

    let mut lowest = Vec::<(&TimeRange, &Heading)>::new();
    for h in hs {
        lowest.push((h.most_recently_started(now).unwrap().to_owned(), h));
    }

    let mut lowest_seen: (&TimeRange, &Heading) = lowest[0];
    for pair in lowest {
        if pair.0.start < lowest_seen.0.start {
            lowest_seen = pair;
        }
    }
    return Some(lowest_seen.1);
}

fn next_prefix_timerange(buf: &str, prefix: &str) -> Option<TimeRange> {
    match buf.find(prefix) {
        Some(c) => {
            return next_timerange(&buf[c + prefix.len()..]);
        }
        None => {
            return None;
        }
    }
}

fn next_timerange(buf: &str) -> Option<TimeRange> {
    let ts_start: &[_] = &['[', '<'];
    let ts_end: &[_] = &['>', ']'];

    match buf.find(ts_start) {
        Some(s) => match buf[s..].find(ts_end) {
            Some(e) => {
                let res = parse_timerange(&buf[s..s + e + 1].to_owned());
                return res;
            }
            None => {
                return None;
            }
        },
        None => {
            return None;
        }
    }
}

// Parse a date and time that may look like:
// <YYYY-MM-DD [dow [time[-endtime]]]>[--<whole thing again>]
fn parse_timerange(buf: &str) -> Option<TimeRange> {
    // is this a double date
    match buf.find("--") {
        Some(beg) => {
            let (start, _) = parse_date_str(&buf[0..beg]);
            let (end, _) = parse_date_str(&buf[beg + 2..]);
            return Some(TimeRange {
                start: start,
                end: end,
            });
        }
        None => {
            let (start, end) = parse_date_str(&buf);
            return Some(TimeRange {
                start: start,
                end: end,
            });
        }
    }
}

// Parse a time, with a time range potentially
// <YYYY-MM-DD [dow [time[-endtime]]][ repeat deadline]>
fn parse_date_str(datestr: &str) -> (NaiveDateTime, NaiveDateTime) {
    let trimmable: &[_] = &['[', '<', '>', ']'];
    let base_str = datestr.trim_matches(trimmable);

    // split string into it's spaced parts
    let base_splits: Vec<&str> = base_str.split(' ').collect();

    // if there is no time specified
    if base_splits.len() < 3
        || base_splits[2].chars().nth(0).unwrap() == '+'
        || base_splits[2].chars().nth(0).unwrap() == '-'
        || base_splits[2].chars().nth(0).unwrap() == '.'
    {
        let start_date = NaiveDate::parse_from_str(base_splits[0], "%Y-%m-%d").unwrap();
        let with_time = start_date.and_hms(0, 0, 0);
        return (with_time, with_time + chrono::Duration::days(1));
    }

    let time_split: Vec<&str> = base_splits[2].split('-').collect();

    // if there is no timerange specified
    if time_split.len() == 1 {
        // point in time
        let start_date = NaiveDateTime::parse_from_str(
            &[base_splits[0], time_split[0]].join(" "),
            "%Y-%m-%d %H:%M",
        )
        .unwrap();
        return (start_date, start_date);
    }

    // now we now we have YYYY-MM-DD Dow HH:MM-HH:MM
    let start_date =
        NaiveDateTime::parse_from_str(&[base_splits[0], time_split[0]].join(" "), "%Y-%m-%d %H:%M")
            .unwrap();
    let end_date =
        NaiveDateTime::parse_from_str(&[base_splits[0], time_split[1]].join(" "), "%Y-%m-%d %H:%M")
            .unwrap();

    return (start_date, end_date);
}

fn parse_single_org_entry(entry: Vec<String>) -> Option<Heading> {
    if entry.len() == 0 {
        return None;
    }
    let mut line = 0;
    let mut firstline = entry[0].to_string();
    let mut level = 0;
    for c in firstline.chars() {
        if c != '*' {
            break;
        }
        level += 1;
    }
    if firstline.len() < level + 1 {
        return None;
    }

    firstline = firstline[level + 1..].to_string();

    let state = if firstline.starts_with("TODO ") {
        firstline = firstline[5..].to_string();
        "TODO"
    } else if firstline.starts_with("DONE ") {
        firstline = firstline[5..].to_string();
        "DONE"
    } else {
        ""
    };

    // parse org tags
    let mut tags = Vec::new();
    if firstline.ends_with(':') {
        // get the last word
        let lastword = firstline.split(' ').last().unwrap().to_owned();
        if lastword.starts_with(':') {
            tags = lastword
                .split(':')
                .filter(|x| !x.is_empty())
                .map(|x| x.to_owned())
                .collect();
        }
    }

    line += 1;

    // check if second line has SCHEDULED/DEADLINE
    if entry.len() < 2 {
        return Some(Heading {
            title: firstline,
            level: level,
            state: state.to_string(),
            tags: tags,
            deadline: None,
            scheduled: None,
            logged: Vec::<TimeRange>::new(),
            logged_active: None,
            timestamps: Vec::<TimeRange>::new(),
        });
    }

    let secondline = &entry[line];
    let scheduled = next_prefix_timerange(&secondline, "SCHEDULED: ");
    let deadline = next_prefix_timerange(&secondline, "DEADLINE: ");
    // skip scheduled/deadline line if there
    if scheduled.is_some() || deadline.is_some() {
        line += 1;
    }

    // parse over properties

    if entry[line] == ":PROPERTIES:" {
        line += 1;
        while entry[line] != ":END:" {
            line += 1;
        }
        // skip final :END:
        line += 1;
    }

    // parse LOGBOOK
    let mut logged = Vec::<TimeRange>::new();
    let mut logged_active: Option<NaiveDateTime> = None;

    if entry.len() > line {
        if entry[line] == ":LOGBOOK:" {
            line += 1;
            while entry[line] != ":END:" {
                if entry[line].starts_with("CLOCK: ") {
                    if entry[line].contains("--") {
                        match next_prefix_timerange(&entry[line], "CLOCK: ") {
                            Some(tr) => {
                                logged.push(tr);
                            }
                            None => (),
                        }
                    } else {
                        // active clock!
                        let (res_time, _) = parse_date_str(&entry[line][7..]);
                        logged_active = Some(res_time);
                    }
                }
                line += 1;
            }
            line += 1;
        }
    }

    // now search for timestamps
    lazy_static! {
        static ref RE: Regex = Regex::new(r"(<\d{4}[^>]+>)(--<\d{4}[^>]+>)?").unwrap();
    }

    let mut timestamps = Vec::<TimeRange>::new();
    while entry.len() > line {
        for mat in RE.find_iter(&entry[line]) {
            match parse_timerange(mat.as_str()) {
                Some(tr) => timestamps.push(tr),
                None => (),
            }
        }
        line += 1;
    }

    return Some(Heading {
        title: firstline,
        level: level,
        state: state.to_string(),
        tags: tags,
        deadline: deadline,
        scheduled: scheduled,
        logged: logged,
        logged_active: logged_active,
        timestamps: timestamps,
    });
}

// read a file, creating entry arrays.
fn parse_org_dates(filename: &str, result_send: mpsc::Sender<Heading>) {
    // read file, and send line by line?
    let file = match fs::File::open(&filename) {
        Ok(f) => f,
        Err(e) => {
            println!("failed to read {}: {}", filename, e);
            panic!("failed to read {}: {}", filename, e);
        }
    };

    let br = BufReader::new(file);
    // create a reference to a new vector.
    let mut rest = Vec::new();
    for line in br.lines().map(|l| l.unwrap()) {
        if line.len() > 0 && line.starts_with("*") {
            if rest.len() > 0 {
                match parse_single_org_entry(rest) {
                    Some(heading) => result_send.send(heading).unwrap(),
                    None => (),
                }
                rest = Vec::new();
            }
        }
        rest.push(line);
    }
    if rest.len() > 0 {
        match parse_single_org_entry(rest) {
            Some(heading) => result_send.send(heading).unwrap(),
            None => (),
        }
    }
}

fn launch_fns(orgfiles: Vec<String>, result_send: mpsc::Sender<Heading>) {
    for of in orgfiles {
        let thread_sender = result_send.clone();
        thread::spawn(move || {
            parse_org_dates(of.as_str(), thread_sender);
        });
    }
}

fn main() {
    let (result_send, result_recv) = mpsc::channel();
    let home_dir = env::var("HOME").unwrap();

    // As far as I can tell, this reads a directory and then returns a
    // list of strings of the contents of that directory. Jesus
    // christ.
    let orgfiles: Vec<String> = fs::read_dir(home_dir.to_string() + "/org/")
        .unwrap()
        .filter(|x| !x.as_ref().unwrap().file_type().unwrap().is_dir())
        .filter(|x| {
            !x.as_ref()
                .unwrap()
                .file_name()
                .to_str()
                .unwrap()
                .starts_with(".")
        })
        .map(|x| {
            home_dir.to_string()
                + "/org/"
                + x.unwrap().path().file_name().unwrap().to_str().unwrap()
        })
        .collect();

    launch_fns(orgfiles, result_send);

    let mut all = Vec::<Heading>::new();
    let mut clocked = Vec::<&Heading>::new();
    let mut action = Vec::<&Heading>::new();
    let mut event = Vec::<&Heading>::new();
    let mut overdue = Vec::<&Heading>::new();
    for message in result_recv {
        all.push(message);
    }
    for message in &all {
        if message.is_clocked_now() {
            clocked.push(&message);
        } else if message.is_action_now() {
            action.push(&message);
        } else if message.is_event_now() {
            event.push(&message);
        } else if message.is_overdue_now() {
            overdue.push(&message);
        }
    }

    print!("^tw()");

    let mut actionstr = String::new();
    if clocked.len() == 1 {
        actionstr += &clocked[0].print_clocked();
    } else if action.len() > 0 {
        match most_recent(&action) {
            Some(h) => actionstr += &h.print_action(),
            None => (),
        }
    } else if overdue.len() > 0 {
        match most_recent(&overdue) {
            Some(h) => actionstr += &h.print_overdue(),
            None => (),
        }
    }

    let mut eventstr = String::new();
    if event.len() > 0 {
        match most_recent(&event) {
            Some(h) => eventstr += &format!("# {}", &h.print_event()),
            None => (),
        }
    }

    if actionstr.len() > 0 {
        print!("{} ", actionstr);
    }
    print!("{}", eventstr);

    println!("^cs()");

    for c in clocked {
        c.print_clocked();
    }
    for o in overdue {
        o.print_overdue();
    }
    for a in action {
        a.print_action();
    }
    for e in event {
        e.print_event();
    }
}
