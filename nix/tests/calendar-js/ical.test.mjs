// The calendar's and contacts' parser (servers/calendar/web/ical.js), run
// under node by checks.calendar-js: times across a change of the clocks, the
// repeat rules people use, and files written back with what they had kept.

import * as I from './ical.mjs';
import assert from 'node:assert/strict';
const ny='America/New_York';
const iso=(ms,tz=ny)=>{const w=I.wall(ms,tz);return `${w.y}-${String(w.m).padStart(2,'0')}-${String(w.d).padStart(2,'0')} ${String(w.h).padStart(2,'0')}:${String(w.mi).padStart(2,'0')}`};
// weekly across DST (Nov 1 2026 DST ends)
let ics=`BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:a\r\nSUMMARY:Climb\\, bouldering\r\nDTSTART;TZID=America/New_York:20261019T183000\r\nDTEND;TZID=America/New_York:20261019T200000\r\nRRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4\r\nEXDATE;TZID=America/New_York:20261026T183000\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n`;
let [ev]=I.events(ics);
assert.equal(ev.summary,'Climb, bouldering');
let occ=I.occurrences(ev,Date.UTC(2026,9,1),Date.UTC(2027,0,1),[],ny).map(o=>iso(o.start));
assert.deepEqual(occ,['2026-10-19 18:30','2026-11-02 18:30','2026-11-09 18:30']);
assert.equal(I.length(ev,ny),90*60000);
// monthly 2nd tuesday until
[ev]=I.events(ics.replace('RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4','RRULE:FREQ=MONTHLY;BYDAY=2TU;UNTIL=20270101T000000Z').replace(/EXDATE.*\r\n/,''));
occ=I.occurrences(ev,Date.UTC(2026,9,1),Date.UTC(2027,6,1),[],ny).map(o=>iso(o.start).slice(0,10));
assert.deepEqual(occ,['2026-11-10','2026-12-08']);
// last friday monthly
[ev]=I.events(ics.replace('RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=4','RRULE:FREQ=MONTHLY;BYDAY=-1FR;COUNT=3').replace(/EXDATE.*\r\n/,''));
occ=I.occurrences(ev,0,Date.UTC(2028,0,1),[],ny).map(o=>iso(o.start).slice(0,10));
assert.deepEqual(occ,['2026-10-30','2026-11-27','2026-12-25']);
// yearly birthday all-day
let bd=`BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:b\r\nDTSTART;VALUE=DATE:19900218\r\nRRULE:FREQ=YEARLY\r\nEND:VEVENT\r\nEND:VCALENDAR`;
[ev]=I.events(bd);
occ=I.occurrences(ev,Date.UTC(2026,0,1),Date.UTC(2028,0,1),[],ny).map(o=>iso(o.start).slice(0,10));
assert.deepEqual(occ,['2026-02-18','2027-02-18']);
assert.equal(I.length(ev),86400000);
// thanksgiving: yearly 4th thursday november
[ev]=I.events(bd.replace('19900218','20201126').replace('FREQ=YEARLY','FREQ=YEARLY;BYMONTH=11;BYDAY=4TH'));
occ=I.occurrences(ev,Date.UTC(2026,0,1),Date.UTC(2028,0,1),[],ny).map(o=>iso(o.start).slice(0,10));
assert.deepEqual(occ,['2026-11-26','2027-11-25']);
// daily interval 2 + override
let ov=`BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:c\r\nDTSTART:20261012T130000Z\r\nDURATION:PT30M\r\nRRULE:FREQ=DAILY;INTERVAL=2;COUNT=3\r\nEND:VEVENT\r\nBEGIN:VEVENT\r\nUID:c\r\nRECURRENCE-ID:20261014T130000Z\r\nDTSTART:20261014T150000Z\r\nDTEND:20261014T153000Z\r\nEND:VEVENT\r\nEND:VCALENDAR`;
let evs=I.events(ov);
assert.equal(evs.length,2);
occ=I.occurrences(evs[0],0,Date.UTC(2027,0,1),[evs[1]],'UTC').map(o=>iso(o.start,'UTC'));
assert.deepEqual(occ,['2026-10-12 13:00','2026-10-16 13:00']);
// weekly mo,we,fr interval 2 wkst mo
[ev]=I.events(ov.split('BEGIN:VEVENT\r\nUID:c\r\nRECURRENCE')[0].replace('FREQ=DAILY;INTERVAL=2;COUNT=3','FREQ=WEEKLY;INTERVAL=2;BYDAY=MO,WE,FR;COUNT=5')+'END:VCALENDAR');
occ=I.occurrences(ev,0,Date.UTC(2027,0,1),[],'UTC').map(o=>iso(o.start,'UTC').slice(0,10));
assert.deepEqual(occ,['2026-10-12','2026-10-14','2026-10-16','2026-10-26','2026-10-28']);
// zoned/wall roundtrip at DST gap edge
const t=I.zoned(2026,3,8,3,30,0,ny); assert.equal(iso(t),'2026-03-08 03:30');
assert.equal(I.zoned(2026,7,1,9,0,0,'Asia/Shanghai'),Date.UTC(2026,6,1,1,0,0));
assert.equal(I.zoneName('Eastern Standard Time'),ny);
assert.equal(I.zoneName('/mozilla.org/20050126_1/America/New_York'),ny);
assert.equal(I.zoneName('Bogus/Zone'),null);
// write & round trip, unknown props kept, folding
const cal=I.newCalendar('u1'); const c=cal.comps[0];
c.props.push({name:'X-GOOGLE-THING',params:{},value:'keep me'});
I.writeEvent(c,{summary:'A very long title '.repeat(8),location:'Room; 4',allDay:false,start:I.zoned(2026,10,20,9,0,0,ny),end:I.zoned(2026,10,20,10,0,0,ny),alarms:[30]},ny);
const out=I.serialize(cal);
assert.ok(out.split('\r\n').every(l=>new TextEncoder().encode(l).length<=75), out);
[ev]=I.events(out);
assert.equal(ev.summary,'A very long title '.repeat(8));
assert.equal(ev.location,'Room; 4');
assert.equal(iso(I.instant(ev.start)),'2026-10-20 09:00');
assert.ok(out.includes('X-GOOGLE-THING:keep me'));
assert.equal(ev.alarms[0].before,30*60000);
assert.ok(out.includes('TRIGGER:-PT30M'), out);
// endBefore
I.writeEvent(c,{summary:'x',allDay:false,start:I.zoned(2026,10,19,9,0,0,ny),end:I.zoned(2026,10,19,10,0,0,ny),rrule:'FREQ=DAILY;COUNT=10'},ny);
I.endBefore(c,{y:2026,m:10,d:22,h:9,mi:0,s:0,tz:ny},ny);
[ev]=I.events(I.serialize(cal));
assert.equal(I.occurrences(ev,0,Date.UTC(2027,0,1),[],ny).length,3);
I.addExdate(c,{y:2026,m:10,d:20,h:9,mi:0,s:0,tz:ny});
[ev]=I.events(I.serialize(cal));
assert.equal(I.occurrences(ev,0,Date.UTC(2027,0,1),[],ny).length,2);
// cards
const vcf=`BEGIN:VCARD\r\nVERSION:3.0\r\nN:Raman;Priya;;Dr.;\r\nFN:Dr. Priya Raman\r\nORG:Harbor Dental;Front desk\r\nitem1.TEL;TYPE=CELL,pref:+1 617 555 0167\r\nEMAIL;TYPE=INTERNET,WORK:p@example.com\r\nADR;TYPE=WORK:;;200 Harbor St;Boston;MA;02110;USA\r\nBDAY:--0218\r\nPHOTO;ENCODING=b;TYPE=JPEG:AAAA\r\nEND:VCARD\r\nBEGIN:VCARD\r\nVERSION:4.0\r\nN:Lee;Jordan;;;\r\nEND:VCARD\r\n`;
const cs=I.cards(vcf);
assert.equal(cs.length,2);
assert.equal(cs[0].fn,'Dr. Priya Raman'); assert.equal(cs[0].org,'Harbor Dental, Front desk');
assert.deepEqual(cs[0].tels,[{kind:'cell',value:'+1 617 555 0167'}]);
assert.deepEqual(cs[0].emails,[{kind:'work',value:'p@example.com'}]);
assert.equal(cs[0].adrs[0].city,'Boston');
assert.deepEqual(I.birthday(cs[0].bday),{y:null,m:2,d:18});
assert.equal(cs[1].fn,'Jordan Lee');
I.writeCard(cs[0].comp,{fn:'Priya Raman',org:'Harbor Dental',title:'',tels:[{kind:'cell',value:'+1 617 555 0100'}],emails:[],adrs:cs[0].adrs,bday:'--0218',note:'Cleanings, March'});
const back=I.serialize(cs[0].comp);
assert.ok(back.includes('PHOTO;ENCODING=b;TYPE=JPEG:AAAA'));
const again=I.cards(back)[0];
assert.equal(again.fn,'Priya Raman'); assert.equal(again.tels[0].value,'+1 617 555 0100'); assert.equal(again.emails.length,0); assert.equal(again.note,'Cleanings, March');
assert.ok(/^[0-9a-f-]{36}$/.test(I.newUid()));
assert.equal(I.durationValue(-90*60000),'-PT1H30M'); assert.equal(I.duration('P1W'),7*86400000); assert.equal(I.durationValue(86400000),'P1D');
console.log('all ok');
