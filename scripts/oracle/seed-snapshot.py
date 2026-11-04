"""Keep only the GitHub fields the disposable Forgejo seed needs."""
import json
import sys

pulls = json.load(sys.stdin)
if not isinstance(pulls, list) or not {1, 2, 4, 5, 6, 7} <= {pr['number'] for pr in pulls}:
    raise SystemExit('oracle seed is missing required PRs')
print(json.dumps([dict(number=pr['number'], title=pr['title'], body=pr['body'],
                       head=dict(ref=pr['head']['ref'], sha=pr['head']['sha']), state=pr['state'],
                       merged_at=pr['merged_at'], draft=pr['draft'])
                  for pr in sorted(pulls, key=lambda pr: pr['number'])], indent=2))
