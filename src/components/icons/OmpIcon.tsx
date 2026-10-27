type OmpIconProps = {
  className?: string;
  size?: number;
};

// Oh My Pi mark, matching the icon used by cc-switch (src/icons/extracted/index.ts).
export function OmpIcon({ className = '', size = 24 }: OmpIconProps) {
  return <svg className={className} width={size} height={size} viewBox="0 0 64 64" fill="none" aria-hidden="true" xmlns="http://www.w3.org/2000/svg">
    <defs>
      <linearGradient id="omp-icon-gradient" x1="0" y1="0" x2="1" y2="1">
        <stop offset="0" stopColor="#ed4abf" />
        <stop offset=".5" stopColor="#9b4dff" />
        <stop offset="1" stopColor="#5ad8e6" />
      </linearGradient>
    </defs>
    <rect width="64" height="64" rx="12" fill="#0f0a14" />
    <path fill="url(#omp-icon-gradient)" d="M14 16h36v8H40v32h-8V24h-6v22h-8V24h-4z" />
  </svg>;
}
