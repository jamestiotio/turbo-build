  $ . ${TESTDIR}/../helpers/setup_example_test.sh with-yarn yarn@1.22.19

# run twice and make sure it works
  $ yarn turbo build lint --output-logs=errors-only
  yarn run v\d+\.\d+\.\d+ (re)
  \$ (.*)node_modules/.bin/turbo build lint --output-logs=errors-only (re)
  \xe2\x80\xa2 Packages in scope: @repo/eslint-config, @repo/typescript-config, @repo/ui, docs, web (esc)
  \xe2\x80\xa2 Running build, lint in 5 packages (esc)
  \xe2\x80\xa2 Remote caching disabled (esc)
  
   Tasks:    5 successful, 5 total
  Cached:    0 cached, 5 total
    Time:\s*[\.0-9ms]+  (re)
  
  Done in [\.0-9]+m?s. (re)

 
  $ yarn turbo build lint --output-logs=errors-only
  yarn run v\d+\.\d+\.\d+ (re)
  \$ (.*)node_modules/.bin/turbo build lint --output-logs=errors-only (re)
  \xe2\x80\xa2 Packages in scope: @repo/eslint-config, @repo/typescript-config, @repo/ui, docs, web (esc)
  \xe2\x80\xa2 Running build, lint in 5 packages (esc)
  \xe2\x80\xa2 Remote caching disabled (esc)
  
   Tasks:    5 successful, 5 total
  Cached:    5 cached, 5 total
    Time:\s*[\.0-9ms]+ >>> FULL TURBO (re)
  
  Done in [\.0-9]+m?s\. (re)


  $ git diff
