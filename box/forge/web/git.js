// Git's front door: the views register themselves, then the first view is
// drawn. Everything they share is git-core.js.

import { whoami, bell, render } from './git-core.js';
import './git-home.js';
import './git-repo.js';
import './git-pulls.js';
import './git-actions.js';
import './git-settings.js';

await whoami().catch(() => null);
bell();
setInterval(bell, 120000);
render();
