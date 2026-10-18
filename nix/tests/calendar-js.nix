# The pages' iCalendar and vCard parser, under node: the one piece of the
# Calendar and Contacts pages whose answers are worked out, not drawn.
{ pkgs, ... }:
pkgs.runCommandLocal "calendar-js"
  {
    nativeBuildInputs = [ pkgs.nodejs ];
  }
  ''
    cp ${../../servers/calendar/web/ical.js} ical.mjs
    cp ${./calendar-js/ical.test.mjs} test.mjs
    # the zones the tests name come from node's own ICU
    TZ=UTC node test.mjs
    touch $out
  ''
