import * as React from 'react';
import { cva, type VariantProps } from 'class-variance-authority';
import { cn } from '@/lib/cn';

// shadcn badge, variant palette trimmed to the pantry's four statuses
// + neutral (connection pills) — full shadcn variant set skipped, add
// when more than these five are needed.
const badgeVariants = cva(
  'inline-flex items-center rounded-full border px-2.5 py-0.5 text-xs font-semibold transition-colors',
  {
    variants: {
      variant: {
        default: 'border-transparent bg-primary/15 text-foreground',
        unopened: 'border-transparent bg-blue-500/15 text-blue-600 dark:text-blue-400',
        opened: 'border-transparent bg-emerald-500/15 text-emerald-600 dark:text-emerald-400',
        expiring: 'border-transparent bg-amber-500/15 text-amber-600 dark:text-amber-400',
        depleted: 'border-transparent bg-neutral-500/15 text-neutral-500 dark:text-neutral-400',
      },
    },
    defaultVariants: { variant: 'default' },
  }
);

export interface BadgeProps
  extends React.ComponentProps<'span'>,
    VariantProps<typeof badgeVariants> {}

function Badge({ className, variant, ...props }: BadgeProps) {
  return (
    <span className={cn(badgeVariants({ variant }), className)} {...props} />
  );
}

export { Badge };
