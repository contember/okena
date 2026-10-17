// Pin the time zone before any test worker starts: the usage pace tests check
// local-time grids (incl. a DST week) against golden values from pace.rs, and
// setting process.env.TZ inside a test file is too late once Date has
// initialised. Workers inherit this from the parent process.
module.exports = () => {
  process.env.TZ = 'Europe/Prague';
};
