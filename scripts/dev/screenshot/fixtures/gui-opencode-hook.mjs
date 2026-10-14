/** Offline OpenCode tool executor around the shipped change-tracker plugin.
 * The caller supplies a temporary module copy with only storage paths isolated.
 * No OpenCode client, network request or paid harness runs.
 */
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {pathToFileURL} from 'node:url';

const [modulePath, repo, name, resultPath] = process.argv.slice(2);
assert(fs.existsSync(path.join(repo, '.amf-gui-opencode-edit-fixture')), 'Refusing a non-fixture checkout');
const file=path.join(repo,name+'.py');
const before=fs.readFileSync(file,'utf8');
const old_string='return used < limit';
const new_string='return used <= limit';
assert(before.includes(old_string));
const {ChangeTracker}=await import(pathToFileURL(modulePath));
const hooks=await ChangeTracker({directory:repo});
assert(hooks['tool.execute.before'], 'The real plugin must be active');
const result={file,before};
try {
  await hooks['tool.execute.before']({tool:'edit',sessionID:'offline-'+name,args:{file_path:file,old_string,new_string}},{});
  // OpenCode performs the tool only after this awaited hook allows it.
  fs.writeFileSync(file,before.replace(old_string,new_string));
  result.status='written';
} catch(error) {
  result.status='rejected';result.error=error.message;
}
result.after=fs.readFileSync(file,'utf8');
const history=path.join(repo,'.amf/change-history.json');
result.history=fs.existsSync(history)?JSON.parse(fs.readFileSync(history,'utf8')).changes:[];
fs.writeFileSync(resultPath,JSON.stringify(result,null,2)+'\n');
