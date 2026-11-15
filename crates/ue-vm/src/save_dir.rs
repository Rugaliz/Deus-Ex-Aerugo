//! The folder of saved games (`Vm::save_dir`): one directory per game, `Save0001`, `Save0002`..., and `QuickSave` for index -1.
//! A directory holds `save.dxs` (the snapshot, see `save.rs`) and `info.txt` (what the load/save screens show). Also the
//! `GameDirectory` natives of the save game type.
use crate::{Value, Vm};
use std::path::PathBuf;

/// What an `XGameDirectory` holds (DeusEx 10012eb0): the listing type (0 maps, 1 saved games), the filter (kept,
/// never used by the listings), the names of the last listing, the temporary save info and the loaded ones.
#[derive(Debug, Clone, Default)]
pub struct GameDir {
    pub kind: i32,
    pub filter: String,
    pub list: Vec<String>,
    pub temp_info: u32,
    /// The save infos `GetSaveInfo`/`GetSaveInfoFromDirectoryIndex` handed out (`loadedSaveInfoPointers`).
    pub loaded: Vec<u32>,
}

/// The order of a Windows directory listing (NTFS, and Wine's sorted listings): names compared upper-cased.
pub fn listing_order(v: &mut [String]) {
    v.sort_by(|a, b| a.to_uppercase().cmp(&b.to_uppercase()));
}

/// The index `GetNewSaveFileIndex` (DeusEx 10017cc0) reads from a listed name: the text after `Save` and ONE more
/// character as a C `atoi` (`Save0012` -> 12; the first digit is skipped, so `Save1234` -> 234), or `None` without `Save`.
pub fn listed_save_index(name: &str) -> Option<i32> {
    let at = name.find("Save")?;
    let rest = &name[at + 4..];
    Some(rest.char_indices().nth(1).map(|(i, _)| crate::natives::c_atoi(&rest[i..])).unwrap_or(0))
}

/// `GetSaveFreeSpace` (DeusEx 10017770): the bytes free to the user on the save folder's drive
/// (`GetDiskFreeSpaceEx`), divided by 1024 as a 64-bit number and then cut to 32 bits; 0 when unknown.
pub fn save_free_space(dir: Option<&std::path::Path>) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let Some(dir) = dir else { return 0 };
        let Ok(path) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return 0 };
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        // SAFETY: a NUL-terminated path and a zeroed `statvfs` the call fills in
        if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
            return 0;
        }
        ((st.f_bavail as u64).wrapping_mul(st.f_frsize as u64) / 1024) as i32
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let Some(dir) = dir else { return 0 };
        let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut avail = 0u64;
        // SAFETY: a NUL-terminated wide path; the two totals the call may skip are null
        if unsafe { win::GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, std::ptr::null_mut(), std::ptr::null_mut()) } == 0 {
            return 0;
        }
        (avail / 1024) as i32
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = dir;
        0
    }
}

#[cfg(windows)]
mod win {
    /// `SYSTEMTIME`
    #[repr(C)]
    #[derive(Default)]
    pub struct SystemTime {
        pub year: u16,
        pub month: u16,
        pub day_of_week: u16,
        pub day: u16,
        pub hour: u16,
        pub minute: u16,
        pub second: u16,
        pub milliseconds: u16,
    }
    #[link(name = "kernel32")]
    extern "system" {
        pub fn GetDiskFreeSpaceExW(dir: *const u16, avail: *mut u64, total: *mut u64, free: *mut u64) -> i32;
        pub fn GetLocalTime(time: *mut SystemTime);
    }
}

pub fn slot_name(index: i32) -> String {
    if index == -1 {
        "QuickSave".to_string()
    } else {
        format!("Save{index:04}")
    }
}

/// What `info.txt` holds (the fields of `DeusExSaveInfo`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SaveMeta {
    pub description: String,
    pub mission_location: String,
    pub map_name: String,
    /// year, month, day, hour, minute, second
    pub time: [i32; 6],
    pub save_count: i32,
    pub save_time: i32,
    pub cheats: bool,
}

impl SaveMeta {
    pub fn to_text(&self) -> String {
        let t = self.time;
        format!(
            "Description={}\nMissionLocation={}\nMapName={}\nTime={} {} {} {} {} {}\nSaveCount={}\nSaveTime={}\nCheats={}\n",
            self.description.replace('\n', " "),
            self.mission_location.replace('\n', " "),
            self.map_name,
            t[0], t[1], t[2], t[3], t[4], t[5],
            self.save_count,
            self.save_time,
            self.cheats as u8
        )
    }

    pub fn from_text(text: &str) -> SaveMeta {
        let mut m = SaveMeta::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            match k {
                "Description" => m.description = v.to_string(),
                "MissionLocation" => m.mission_location = v.to_string(),
                "MapName" => m.map_name = v.to_string(),
                "Time" => {
                    for (i, p) in v.split_whitespace().take(6).enumerate() {
                        m.time[i] = p.parse().unwrap_or(0);
                    }
                }
                "SaveCount" => m.save_count = v.parse().unwrap_or(0),
                "SaveTime" => m.save_time = v.parse().unwrap_or(0),
                "Cheats" => m.cheats = v.trim() == "1",
                _ => {}
            }
        }
        m
    }
}

/// The current local time as year, month, day, hour, minute, second: `appSystemTime` (Windows `GetLocalTime`).
pub fn now_civil() -> [i32; 6] {
    #[cfg(windows)]
    {
        let mut t = win::SystemTime::default();
        // SAFETY: a `SYSTEMTIME` the call fills in
        unsafe { win::GetLocalTime(&mut t) };
        [t.year, t.month, t.day, t.hour, t.minute, t.second].map(i32::from)
    }
    #[cfg(not(windows))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        #[cfg(unix)]
        {
            let t: libc::time_t = secs as libc::time_t;
            let mut tm: libc::tm = unsafe { std::mem::zeroed() };
            // SAFETY: a valid time and a zeroed `tm` that `localtime_r` fills in
            if !unsafe { libc::localtime_r(&t, &mut tm) }.is_null() {
                return [tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec];
            }
        }
        utc_civil(secs)
    }
}

/// Without a local time zone: UTC, the civil date from days since 1970-01-01.
#[cfg(not(windows))]
fn utc_civil(secs: i64) -> [i32; 6] {
    let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    [year as i32, month as i32, day as i32, (rem / 3600) as i32, (rem % 3600 / 60) as i32, (rem % 60) as i32]
}

impl Vm {
    pub fn save_slot_dir(&self, index: i32) -> Option<PathBuf> {
        self.save_dir.as_ref().map(|d| d.join(slot_name(index)))
    }

    /// The numbered saved games, oldest index first (the quick save is not among them).
    pub fn save_slots(&self) -> Vec<i32> {
        let Some(dir) = &self.save_dir else { return Vec::new() };
        let mut v: Vec<i32> = std::fs::read_dir(dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_prefix("Save")).and_then(|n| n.parse::<i32>().ok()))
                    .filter(|&i| i > 0 && self.save_slot_dir(i).map(|d| d.join("save.dxs").exists()).unwrap_or(false))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    pub fn read_save_meta(&self, index: i32) -> Option<SaveMeta> {
        let dir = self.save_slot_dir(index)?;
        if !dir.join("save.dxs").exists() {
            return None;
        }
        Some(SaveMeta::from_text(&std::fs::read_to_string(dir.join("info.txt")).unwrap_or_default()))
    }

    /// `make_save_info` for tests and tools: the object id (0 for none).
    pub fn make_save_info_pub(&mut self, index: i32) -> u32 {
        match self.make_save_info(index) {
            Value::Obj(o) => o,
            _ => 0,
        }
    }

    /// A `DeusExSaveInfo` object describing saved game `index` (`None` when there is no such game).
    fn make_save_info(&mut self, index: i32) -> Value {
        let Some(meta) = self.read_save_meta(index) else { return Value::Obj(0) };
        let Some(class) = self.reg.class_by_name("DeusExSaveInfo") else { return Value::Obj(0) };
        let Ok(obj) = self.new_object(class) else { return Value::Obj(0) };
        for (n, v) in [("Year", meta.time[0]), ("Month", meta.time[1]), ("Day", meta.time[2]), ("Hour", meta.time[3]), ("Minute", meta.time[4]), ("Second", meta.time[5]), ("DirectoryIndex", index), ("saveCount", meta.save_count), ("saveTime", meta.save_time)] {
            let _ = self.set_prop(obj, n, 0, Value::Int(v));
        }
        for (n, v) in [("Description", meta.description), ("MissionLocation", meta.mission_location), ("MapName", meta.map_name)] {
            let _ = self.set_prop(obj, n, 0, Value::Str(v));
        }
        let _ = self.set_prop(obj, "bCheatsEnabled", 0, Value::Bool(meta.cheats));
        // the picture `SaveGame` took (`Snapshot`), when the save has one
        if let Some(tex) = self.save_slot_dir(index).and_then(|d| self.load_save_snapshot(&d.join(crate::snapshot::SAVE_FILE))) {
            let _ = self.set_prop(obj, "Snapshot", 0, Value::Obj(tex));
        }
        Value::Obj(obj)
    }

    fn note_loaded_info(&mut self, dir: u32, v: &Value) {
        if let Value::Obj(o) = *v {
            if o != 0 {
                self.ui.game_dirs.entry(dir).or_default().loaded.push(o);
            }
        }
    }

    fn delete_save_info(&mut self, dir: u32, info: u32) {
        let Some(d) = self.ui.game_dirs.get_mut(&dir) else { return };
        let Some(i) = d.loaded.iter().position(|&o| o == info && info != 0) else { return };
        d.loaded.remove(i);
        self.critical_delete(info);
    }

    /// The `GameDirectory` natives about saved games' contents; `None` for the ones `game_directory` answers.
    pub(crate) fn save_directory(&mut self, this: u32, name: &str, a: &mut [Value]) -> Option<Value> {
        let int = |i: usize| a.get(i).and_then(Value::as_i32).unwrap_or(0);
        Some(match name {
            "GetSaveInfo" => {
                let v = self.make_save_info(int(0));
                self.note_loaded_info(this, &v);
                v
            }
            // (DeusEx 10018000) the info of the listed game `i`: its folder's name read back as the port's index
            "GetSaveInfoFromDirectoryIndex" => {
                let listed = usize::try_from(int(0)).ok().and_then(|i| self.ui.game_dirs.get(&this).and_then(|d| d.list.get(i)).cloned());
                let v = match listed.and_then(|n| n.get(4..).and_then(|d| d.parse::<i32>().ok())) {
                    Some(index) => self.make_save_info(index),
                    None => Value::Obj(0),
                };
                self.note_loaded_info(this, &v);
                v
            }
            // (DeusEx 100179d0) the files of `Save<index>` (-1: `QuickSave`), each one's size in whole KB, summed
            "GetSaveDirectorySize" => {
                let kb = self
                    .save_slot_dir(int(0))
                    .and_then(|d| std::fs::read_dir(d).ok())
                    .map(|rd| {
                        rd.flatten()
                            .filter_map(|e| e.metadata().ok())
                            .filter(|m| m.is_file())
                            .fold(0i32, |sum, m| sum.wrapping_add((m.len() as i32) / 1024))
                    })
                    .unwrap_or(0);
                Value::Int(kb)
            }
            // (DeusEx 10018170) only an info this directory handed out: destroyed and forgotten
            "DeleteSaveInfo" => {
                if let Some(Value::Obj(info)) = a.first() {
                    self.delete_save_info(this, *info);
                }
                Value::Void
            }
            // (DeusEx 10018240) deletes while stepping its index over the shrinking list: every other info is
            // destroyed, the rest are only forgotten
            "PurgeAllSaveInfo" => {
                let mut i = 0;
                while let Some(&info) = self.ui.game_dirs.get(&this).and_then(|d| d.loaded.get(i)) {
                    self.delete_save_info(this, info);
                    i += 1;
                }
                if let Some(d) = self.ui.game_dirs.get_mut(&this) {
                    d.loaded.clear();
                }
                Value::Void
            }
            _ => return None,
        })
    }
}
