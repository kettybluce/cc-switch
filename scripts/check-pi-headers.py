import json
p = '/home/<user>/.pi/agent/models.json'
d = json.load(open(p, encoding='utf-8'))
for k, v in d['providers'].items():
    print(f"{k:<10} headers={v.get('headers')}  api={v.get('api')}")
