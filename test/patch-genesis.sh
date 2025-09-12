PWD_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
. "$PWD_DIR/.env"
#if not $NEED_PATCH_WTIH_MDBX != true exit 0
if [ "$NEED_PATCH_GENESIS" != "true" ]; then
  echo "Skipping patching genesis.json with mdbx.dat as $NEED_PATCH_GENESIS is not set to true."
  exit 0
fi

# patch with mdbx.dat using hack
# check whether $MDBX_FILE exists, if not, output
if [ -f "$MDBX_FILE" ]; then
  echo "✅ Found mdbx.dat at $MDBX_FILE, proceeding to patch genesis.json"
else
  echo "❌ mdbx.dat not found at $MDBX_FILE, cannot patch"
  exit 1
fi

RAMFS_DIR=$PWD_DIR/ramfs
echo "RAMFS_DIR is $RAMFS_DIR"

mkdir -p $RAMFS_DIR || echo "$RAMFS_DIR already exists"
#if not macos then
if [[ "$OSTYPE" != "darwin"* ]]; then
  mount -t ramfs ramfs $RAMFS_DIR
  echo "Mounted ramfs at $RAMFS_DIR"
fi
cp "$MDBX_FILE" $RAMFS_DIR/mdbx.dat

cp config-op/genesis.json $RAMFS_DIR/genesis.json
mv config-op/genesis.json config-op/genesis.json.bak
#check if $(which hack) exists

if ! command -v hack &> /dev/null; then
  echo "❌ hack command not found, please install it from"
  exit 1
fi
hack -action migrateGenesis -chaindata $RAMFS_DIR -input $RAMFS_DIR/genesis.json -output config-op/genesis.json -log-level info

if [[ "$OSTYPE" != "darwin"* ]]; then
  umount  $RAMFS_DIR
  echo "un mounted ramfs at $RAMFS_DIR"
fi
