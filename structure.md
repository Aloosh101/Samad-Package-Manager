 توضيح القرارات الأساسية
✅ نظام العزل
نعم، فقط بطلب المستخدم:
spm install nginx ← يثبت في المسارات القياسية (/usr/bin, /usr/lib) مثل apt/dnf
spm install --sandbox nginx ← يعزل في البيئة المخصصة
spm install --sandbox=full nginx ← عزل كامل (namespace)
AI يقترح فقط العزل إذا رأى تعارضاً، ولا يفعّله أبداً بدون موافقة المستخدم
✅ التخزين الافتراضي
نعم، مسارات قياسية مثل .deb و .rpm:
/usr/bin/ ← الملفات التنفيذية
/usr/lib/ ← المكتبات
/etc/ ← الإعدادات
لا يستخدم /nix/store ما لم يطلب المستخدم العزل
📐 تفاصيل التصميم الكاملة
1. CLI — واجهة الأوامر
spm install <pkg>                 ← تثبيت قياسي (مسارات قياسية)
spm install --sandbox <pkg>       ← تثبيت معزول
spm install --sandbox=strict <pkg> ← عزل كامل (namespaces)
spm install --convert-only <file> ← تحويل .deb/.rpm → .sam بدون تثبيت
spm install --prefer-newest <pkg> ← أحدث الإصدارات (بغض النظر عن أولوية الريبو)
spm install --stable-debian <pkg> ← استقرار + ديبيان (قسري)
spm install --newest-redhat <pkg> ← حداثة + ريدهات (قسري)

spm remove <pkg>                  ← إزالة
spm purge <pkg>                   ← إزالة + حذف الإعدادات

spm update                        ← تحديث بيانات المستودعات
spm upgrade                       ← ترقية جميع الحزم
spm upgrade <pkg>                 ← ترقية حزمة محددة
spm upgrade --prefer-newest       ← ترقية بأحدث الإصدارات
spm upgrade --stable-debian       ← ترقية باستقرار + ديبيان
spm upgrade --newest-redhat       ← ترقية بحداثة + ريدهات

spm search <query>                ← بحث
spm info <pkg>                    ← معلومات حزمة
spm files <pkg>                   ← قائمة ملفات الحزمة
spm depends <pkg>                 ← تبعيات الحزمة
spm rdepends <pkg>                ← الحزم التي تعتمد على هذه

spm history                       ← سجل المعاملات (مثل dnf history)
spm history undo <id>             ← التراجع عن معاملة

spm snapshot create               ← إنشاء snapshot Btrfs (إن وُجد)
spm snapshot rollback <id>        ← استعادة snapshot

spm analyze                       ← تحليل النظام بالكامل
spm analyze orphan                ← حزم يتيمة فقط
spm analyze conflicts             ← تعارضات فقط
spm analyze trace <binary>        ← محاكاة تشغيل ثنائي معين

spm ps                            ← عمليات تستخدم ملفات محذوفة (مثل zypper ps)

spm sandbox list                  ← قائمة الحزم المعزولة
spm sandbox run <pkg> <cmd>       ← تشغيل أمر داخل sandbox

spm config                        ← عرض الإعدادات
spm config set <key> <val>        ← تعديل إعداد
       prefer_newest = true/false  ← افتراضي: حداثة أم استقرار (بدون --prefer-newest)
2. نظام التخزين — هيكل المجلدات
/var/lib/spm/
├── packages/           ← ذاكرة الحزم المحملة (.sam files)
├── metadata.db         ← SQLite: كل الحزم المثبتة وملفاتها
├── transactions.db     ← SQLite: سجل المعاملات
├── files/              ← قوائم الملفات لكل حزمة
│   ├── nginx-1.27.0    ← قائمة بكل ملف يخص الحزمة (لـ rollback)
│   └── openssl-3.0.15
└── sandboxes/          ← إذا استُخدم العزل
    ├── nginx/
    │   ├── usr/
    │   ├── etc/
    │   └── manifest.sam
    └── ...

/var/cache/spm/
├── archives/           ← الحزم المحملة (.deb, .rpm, .sam)
└── repos/              ← metadata المستودعات

/etc/spm/
├── spm.conf            ← الإعدادات العامة
└── repos.d/            ← تعريفات المستودعات
    ├── ubuntu.list
    ├── fedora.list
    └── samad.list
3. قاعدة البيانات — هيكل SQLite
-- سجل التثبيتات (للتراجع)
CREATE TABLE transactions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action TEXT NOT NULL,        -- install, remove, upgrade
    timestamp TEXT NOT NULL,
    user TEXT NOT NULL,
    status TEXT NOT NULL,        -- completed, undone, failed
    packages TEXT NOT NULL,      -- JSON array
    snapshot_id TEXT             -- Btrfs snapshot ID إن وُجد
);

-- كل ملف يخص أي حزمة (للتراجع اليدوي)
CREATE TABLE files (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    transaction_id INTEGER NOT NULL,
    package TEXT NOT NULL,
    filepath TEXT NOT NULL,
    hash TEXT NOT NULL,          -- BLAKE3 hash قبل التعديل
    action TEXT NOT NULL,        -- created, modified, deleted
    FOREIGN KEY (transaction_id) REFERENCES transactions(id)
);

-- الحزم المثبتة حالياً
CREATE TABLE installed_packages (
    name TEXT PRIMARY KEY,
    version TEXT NOT NULL,
    format TEXT NOT NULL,         -- deb, rpm, sam
    install_type TEXT NOT NULL,   -- native, sandbox
    manifest TEXT,                -- manifest.json كامل
    install_date TEXT NOT NULL,
    source_repo TEXT
);
كيف يعمل التراجع (rollback) بدون Nix Store؟
spm install nginx
  ↓
يسجل في transactions.db: { id: 42, action: "install", packages: ["nginx"] }
يسجل في files.db: كل ملف جديد أنشأه nginx مع BLAKE3 hash
  ↓
spm history undo 42
  ↓
يقرأ files.db: كل الملفات المسجلة في المعاملة 42
يستعيد النسخ الأصلية من /var/cache/spm/archives/ (إن وُجدت)
أو يحذف الملفات الجديدة
  ↓
إن وُجد Btrfs → يستخدم snapshot بدلاً من ذلك (أسرع وأضمن)
4. دعم Btrfs Snapshots (اختياري — عند التوفر)
عند التثبيت، إذا كان النظام يستخدم Btrfs:
  1. إنشاء snapshot @ قبل التثبيت (مثل openSUSE)
  2. تثبيت الحزمة
  3. ربط الـ snapshot مع transaction.id

عند التراجع:
  spm history undo 42
  → Btrfs rollback snapshot ← أسرع وأضمن
  → يعيد النظام لحالة ما قبل التثبيت بالكامل

إذا لم يكن Btrfs موجوداً:
  → التراجع عبر قاعدة البيانات والملفات المحفوظة
5. مصادر الحزم (Parasite Mode بالتفصيل)
/etc/spm/repos.d/ubuntu.list:
  source = "apt"
  distro = "ubuntu"
  codename = "noble"
  components = ["main", "universe", "multiverse"]
  mirrors = ["http://archive.ubuntu.com/ubuntu"]

/etc/spm/repos.d/fedora.list:
  source = "dnf"
  distro = "fedora"
  release = "40"
  repos = ["fedora", "updates", "rpmfusion-free"]

/etc/spm/repos.d/samad.list:
  source = "native"          ← المرحلة 2
  url = "https://repo.samad.iq/stable"
عملية التثبيت من apt repo:
spm install nginx
  ↓
1. يبحث في ubuntu.list عن nginx
2. يسحب metadata من مستودعات Ubuntu
3. يحل التبعيات (SAT solver)
4. يسحب .deb من mirror
5. يقرأ manifest من .deb
6. يسجل في SQLite
7. يفك الضغط مباشرة إلى /usr/bin, /usr/lib, /etc
8. يسجل transaction.id = 42
9. يسجل قائمة الملفات مع hashes
6. صيغة .sam — تفاصيل كاملة
.sam هو tar بثلاثة أقسام (مضغوط بـ zstd متوازي):
[رأس ثابت 4 بايت: "SAM1"]

[manifest.json بطول محدد]
{
  "name": "nginx",
  "version": "1.27.0",
  "architecture": "amd64",
  "maintainer": "spm@samad.iq",
  "description": "...",
  "dependencies": [
    {"name": "libssl3", "version": ">=3.0.0", "source": "system"},
    {"name": "zlib1g", "version": ">=1.2.0", "source": "system"}
  ],
  "conflicts": ["apache2"],
  "provides": ["nginx-full", "httpd"],
  "recommends": ["ca-certificates"],
  "install_size": 5242880,
  "format_version": 1,
  "source": {                         ← مصدر الحزمة الأصلية
    "original_format": "deb",
    "original_package": "nginx_1.27.0_amd64.deb",
    "repo": "ubuntu noble main",
    "hash_original": "abc123def..."
  },
  "ai_metadata": {                    ← بيانات من AI عند التحويل
    "converted": true,
    "conversion_date": "2026-06-01",
    "dependencies_verified": true,
    "conflicts_resolved": ["libssl1.1"],
    "sandbox_required": false
  },
  "signature": {
    "algorithm": "Ed25519",
    "key_id": "spm@samad.iq",
    "value": "base64(signature)"
  }
}

[data.tar.zst]
→ usr/bin/nginx
→ usr/lib/nginx/...
→ etc/nginx/nginx.conf
→ ...

[meta.tar.zst]
→ preinst.sh, postinst.sh, prerm.sh, postrm.sh
7. AI Engine — التفاصيل الكاملة لوظائفه
spm analyze
  ↓
① **فحص الحزم اليتيمة**:
   - يقرأ SQLite: كل الحزم المثبتة
   - يبني dependency graph كاملاً
   - يحدد packages بدون أي package آخر يعتمد عليها
   - يعرضها مع الحجم واقتراح الحذف

② **فحص التعارضات المخفية**:
   - يبحث عن مكتبات SONAME متكررة في /usr/lib
   - يبحث عن ملفات مملوكة لحزم متعددة
   - يحلل broken symlinks

③ **تحليل التبعيات المعقدة**:
   - يبحث عن dependency cycles (حلقات)
   - يحلل upgrade paths الآمنة
   - يقترح مسار التحديث الأمثل (أقل تغيير، أكثر استقرار)

spm analyze trace /usr/bin/nginx
  ↓
④ **محاكاة التشغيل**:
   - يأخذ نسخة من الثنائي والمكتبات إلى /tmp/.spm-trace/
   - يشغّل مع LD_LIBRARY_PATH مضبوط
   - يرصد missing libraries عبر strace أو LD_PRELOAD hook
   - يقترح: "ينقصك libpcre.so.3 → ثبّت libpcre3"
8. الـ Sandbox — التفاصيل
يُفعّل فقط عند طلب المستخدم بـ --sandbox:
spm install --sandbox nginx
  ↓
  --sandbox (بدون مستوى) = المستوى 1
  --sandbox=strict         = المستوى 2
  --sandbox=full           = المستوى 3

المستوى 1 — Symlink Farm:
  /var/lib/spm/sandboxes/nginx/
  ├── usr/bin/nginx  (ملف حقيقي)
  └── usr/lib/libssl.so.3 → /usr/lib/x86_64-linux-gnu/libssl.so.3 (symlink)
  يعمل التطبيق بشكل طبيعي، لكنه يرى فقط المكتبات التي أذنت لها

المستوى 2 — Namespace (Bubblewrap):
  bwrap \
    --ro-bind /var/lib/spm/sandboxes/nginx/usr /usr \
    --ro-bind /usr/lib /usr/lib (أساسي) \
    --proc /proc \
    --dev /dev \
    --unshare-net \
    nginx

المستوى 3 — Full Container (Bubblewrap + OverlayFS + Seccomp):
  كالمستوى 2 + قيود إضافية:
  - شبكة معزولة تماماً
  - Seccomp يمنع syscalls خطيرة
  - نظام ملفات وهمي (tmpfs) لـ /tmp
  - ممنوع الوصول إلى /etc الحقيقي (يستخدم etc وهمي من sandbox)
9. الـ spm ps — مثل zypper ps
spm ps
  ↓
يفحص /proc/<pid>/maps لجميع العمليات الجارية
يقارن مسارات .so مع /var/lib/spm/metadata.db
إذا كانت عملية تستخدم مكتبة من حزمة تم تحديثها أو حذفها ← يحذّر
  ┌────────────────────────────────────────────┐
  │ PID  │ PROCESS   │ DELETED LIB             │
  ├────────────────────────────────────────────┤
  │ 1234 │ nginx     │ libssl.so.3 (old 3.0.1) │
  │ 5678 │ postgres  │ libpq.so.5 (deleted)    │
  └────────────────────────────────────────────┘
  الاقتراح: systemctl restart nginx postgresql
📋 الخلاصة — التدفق الكامل لأمر spm install nginx
المستخدم: spm install nginx
  │
  ├─→ يبحث في repos.d/ (ubuntu → fedora → samad)
  ├─→ يجد nginx في مستودع Ubuntu noble
  ├─→ يسحب metadata + dependency graph
  ├─→ SAT solver يحل التبعيات
  │     ├─→ libc6, libssl3, zlib1g ← مثبتة مسبقاً ✓
  │     └─→ libpcre3 ← سيُثبّت
  ├─→ يسأل المستخدم: "سيتم تثبيت: nginx + libpcre3. متابعة؟ [Y/n]"
  ├─→ [Y]
  ├─→ يسحب .deb من mirror (متوازي مع تعدد خيوط)
  ├─→ يتحقق من التوقيع والتكامل (BLAKE3 + Ed25519)
  ├───┬─→ [تثبيت قياسي]
  │   ├─→ يسجل transaction في SQLite
  │   ├─→ يسجل ملفات nginx مع hashes
  │   ├─→ يفك الضغط: nginx → /usr/sbin/nginx
  │   ├─→ يفك الضغط: libpcre3 → /usr/lib/x86_64-linux-gnu/
  │   └─→ ينفذ postinst scripts
  │
  ├─→ [في حالة تعارض]
  │   ├─→ AI يحلل التعارض
  │   ├─→ "وجدت تعارض: libssl1.1 vs libssl3"
  │   ├─→ الخيارات:
  │   │    1. تثبيت مع خفض nginx (مستقر)
  │   │    2. تثبيت مع عزل nginx (--sandbox)
  │   │    3. تحويل إلى .sam مع إعادة ربط المكتبات
  │   └─→ المستخدم يختار → ينفذ
  │
  └─→ Done ✓
  spm ps → يفحص إن كانت nginx قيد التشغيل تحتاج إعادة تشغيل
سؤال لك: هل تريد التعمق في أي جزء معين؟ مثلاً:
هيكل قاعدة SQLite بالكامل (جداول أكثر تفصيلاً)؟
آلية الـ SAT solver + AI بالتفصيل الرياضي؟
صيغة .sam وطريقة التحويل من .deb/.rpm خطوة بخطوة؟
كيف سيعمل spm analyze مع tracing؟
