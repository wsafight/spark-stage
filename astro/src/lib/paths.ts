const base = import.meta.env.BASE_URL;

export function withBase(pathname = '/'): string {
  if (pathname === '/' || pathname === '') {
    return base;
  }

  const relative = pathname.replace(/^\/+/, '').replace(/\/+$/, '');
  return `${base}${relative}/`;
}
