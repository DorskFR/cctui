export function rewriteGithubUrl(input: string, origin: string): string | null {
  const pull = /github\.com\/([^/]+)\/([^/]+)\/pull\/(\d+)/.exec(input);
  if (pull) {
    return `${origin.replace(/\/$/, "")}/${pull[1]}/${pull[2]}/pull/${pull[3]}`;
  }
  return null;
}

export function bookmarkletSource(origin: string): string {
  const base = origin.replace(/\/$/, "");
  const body =
    `var m=location.href.match(/github\\.com\\/([^\\/]+)\\/([^\\/]+)\\/pull\\/(\\d+)/);` +
    `if(m){location.href=${JSON.stringify(base)}+'/'+m[1]+'/'+m[2]+'/pull/'+m[3];}` +
    `else{alert('Not a GitHub pull request URL');}`;
  return `javascript:(function(){${body}})();`;
}
