# run

```
python3 -m venv venv
source venv/bin/activate
pip install matplotlib web3

pip install -r requirements.txt

python -c "import matplotlib; print(matplotlib.__version__)"
deactivate

python query_data.py 23738651 23739651
```