"use client"

import { useRef, useState, useCallback } from "react"
import { Moon, Sun } from "lucide-react"
import { flushSync } from "react-dom"
import { motion, AnimatePresence } from "framer-motion"
import { cn } from "../../lib/utils"

export type ThemeTransitionType = "horizontal" | "vertical" | "circle"

type AnimatedThemeToggleButtonProps = {
  type?: ThemeTransitionType
  className?: string
}

function useThemeState() {
  const [darkMode, setDarkMode] = useState(() => {
    if (typeof window === "undefined") return false;
    const isDark = localStorage.getItem("theme") === "dark";
    if (isDark) {
      document.documentElement.classList.add("dark");
      return true;
    }
    return document.documentElement.classList.contains("dark");
  })

  return [darkMode, setDarkMode] as const
}

function triggerThemeTransition(type: ThemeTransitionType, event?: React.MouseEvent) {
  if (type === "circle" && event) {
    const x = event.clientX;
    const y = event.clientY;
    const endRadius = Math.hypot(
      Math.max(x, innerWidth - x),
      Math.max(y, innerHeight - y)
    );
    
    document.documentElement.animate(
      {
        clipPath: [
          `circle(0px at ${x}px ${y}px)`,
          `circle(${endRadius}px at ${x}px ${y}px)`
        ]
      },
      {
        duration: 700,
        easing: "ease-in-out",
        pseudoElement: "::view-transition-new(root)",
      }
    )
  } else if (type === "horizontal") {
    document.documentElement.animate(
      { clipPath: ["inset(50% 0 50% 0)", "inset(0 0 0 0)"] },
      { duration: 700, easing: "ease-in-out", pseudoElement: "::view-transition-new(root)" }
    )
  } else if (type === "vertical") {
    document.documentElement.animate(
      { clipPath: ["inset(0 50% 0 50%)", "inset(0 0 0 0)"] },
      { duration: 700, easing: "ease-in-out", pseudoElement: "::view-transition-new(root)" }
    )
  }
}

export const AnimatedThemeToggleButton = ({
  type = "circle",
  className
}: AnimatedThemeToggleButtonProps) => {
  const buttonRef = useRef<HTMLButtonElement>(null)
  const [darkMode, setDarkMode] = useThemeState()

  const handleToggle = useCallback(async (e: React.MouseEvent) => {
    if (!buttonRef.current) return

    // Fallback if view transition API isn't supported
    if (!document.startViewTransition) {
        const toggled = !darkMode
        setDarkMode(toggled)
        document.documentElement.classList.toggle("dark", toggled)
        localStorage.setItem("theme", toggled ? "dark" : "light")
        return;
    }

    await document.startViewTransition(() => {
      flushSync(() => {
        const toggled = !darkMode
        setDarkMode(toggled)
        document.documentElement.classList.toggle("dark", toggled)
        localStorage.setItem("theme", toggled ? "dark" : "light")
      })
    }).ready

    triggerThemeTransition(type, e)
  }, [darkMode, type, setDarkMode])

  return (
    <button
      ref={buttonRef}
      onClick={handleToggle}
      aria-label={`Toggle theme`}
      type="button"
      className={cn(
        "flex items-center justify-center p-2 rounded-full outline-none focus:outline-none cursor-pointer transition-colors duration-300 relative z-50 bg-transparent opacity-80 hover:opacity-100",
        className
      )}
      style={{ width: 44, height: 44 }}
    >
      <AnimatePresence mode="wait" initial={false}>
        {darkMode ? (
          <motion.span
            key="sun"
            initial={{ opacity: 0, scale: 0.55, rotate: 25 }}
            animate={{ opacity: 1, scale: 1, rotate: 0 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.33 }}
            className="text-brand-orange"
          >
            <Sun size={20} />
          </motion.span>
        ) : (
          <motion.span
            key="moon"
            initial={{ opacity: 0, scale: 0.55, rotate: -25 }}
            animate={{ opacity: 1, scale: 1, rotate: 0 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.33 }}
            className="text-brand-red"
          >
            <Moon size={20} />
          </motion.span>
        )}
      </AnimatePresence>
    </button>
  )
}
