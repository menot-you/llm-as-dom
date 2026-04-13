"use client";

class Pixel {
  width: number;
  height: number;
  ctx: CanvasRenderingContext2D;
  x: number;
  y: number;
  color: string;
  speed: number;
  size: number;
  sizeStep: number;
  minSize: number;
  maxSizeInteger: number;
  maxSize: number;
  delay: number;
  counter: number;
  counterStep: number;
  isIdle: boolean;
  isReverse: boolean;
  isShimmer: boolean;

  constructor(
    canvas: HTMLCanvasElement,
    context: CanvasRenderingContext2D,
    x: number,
    y: number,
    color: string,
    speed: number,
    _delay: number
  ) {
    this.width = canvas.width;
    this.height = canvas.height;
    this.ctx = context;
    this.x = x;
    this.y = y;
    this.color = color;
    this.speed = this.getRandomValue(0.1, 0.9) * speed;
    this.minSize = 0.5;
    this.maxSizeInteger = 2;
    this.maxSize = this.getRandomValue(this.minSize, this.maxSizeInteger);
    
    // INSTANT SPAWN: Start at random sizes instead of 0
    this.size = this.getRandomValue(this.minSize, this.maxSize); 
    this.sizeStep = Math.random() * 0.4;
    this.delay = 0; // No loading delay cascade
    this.counter = 0;
    this.counterStep = Math.random() * 4 + (this.width + this.height) * 0.01;
    this.isIdle = false;
    
    // MIXED DIRECTIONS: Start going up or down randomly
    this.isReverse = Math.random() > 0.5; 
    
    // ALWAYS SHIMMER: skip the 'appear' loading transition
    this.isShimmer = true; 
  }

  getRandomValue(min: number, max: number) {
    return Math.random() * (max - min) + min;
  }

  draw() {
    const centerOffset = this.maxSizeInteger * 0.5 - this.size * 0.5;
    this.ctx.fillStyle = this.color;
    this.ctx.fillRect(
      this.x + centerOffset,
      this.y + centerOffset,
      this.size,
      this.size
    );
  }

  appear() {
    this.isIdle = false;
    // Instantly tick simulation
    this.shimmer();
    this.draw();
  }

  disappear() {
    this.isShimmer = false;
    this.counter = 0;

    if (this.size <= 0) {
      this.isIdle = true;
      return;
    } else {
      this.size -= 0.1;
    }

    this.draw();
  }

  shimmer() {
    if (this.size >= this.maxSize) {
      this.isReverse = true;
    } else if (this.size <= this.minSize) {
      this.isReverse = false;
    }

    if (this.isReverse) {
      this.size -= this.speed;
    } else {
      this.size += this.speed;
    }
  }
}

class PixelCanvasElement extends HTMLElement {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D | null;
  private pixels: Pixel[] = [];
  private animation: number | null = null;
  private timeInterval: number = 1000 / 60;
  private timePrevious: number = performance.now();
  private reducedMotion: boolean;
  private _initialized: boolean = false;
  private _resizeObserver: ResizeObserver | null = null;

  constructor() {
    super();
    this.canvas = document.createElement("canvas");
    this.ctx = this.canvas.getContext("2d");
    this.reducedMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)"
    ).matches;

    const shadow = this.attachShadow({ mode: "open" });
    const style = document.createElement("style");
    style.textContent = `
      :host {
        display: grid;
        inline-size: 100%;
        block-size: 100%;
        overflow: hidden;
      }
    `;
    shadow.appendChild(style);
    shadow.appendChild(this.canvas);
  }

  get colors() {
    return this.dataset.colors?.split(",") || ["#f8fafc", "#f1f5f9", "#cbd5e1"];
  }

  get gap() {
    const value = Number(this.dataset.gap) || 5;
    return Math.max(4, Math.min(50, value));
  }

  get speed() {
    const value = Number(this.dataset.speed) || 35;
    return this.reducedMotion ? 0 : Math.max(0, Math.min(100, value)) * 0.001;
  }

  get noFocus() {
    return this.hasAttribute("data-no-focus");
  }

  get variant() {
    return this.dataset.variant || "default";
  }

  connectedCallback() {
    if (this._initialized) return;
    this._initialized = true;

    requestAnimationFrame(() => {
      this.handleResize();

      const ro = new ResizeObserver((entries) => {
        if (!entries.length) return;
        requestAnimationFrame(() => this.handleResize());
      });
      ro.observe(this);
      this._resizeObserver = ro;

      // Always active! Trigger appearance immediately.
      this.handleAnimation("appear");
    });
  }

  disconnectedCallback() {
    this._initialized = false;
    this._resizeObserver?.disconnect();

    if (this.animation) {
      cancelAnimationFrame(this.animation);
      this.animation = null;
    }
  }

  handleResize() {
    if (!this.ctx || !this._initialized) return;

    const rect = this.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    const width = Math.floor(rect.width);
    const height = Math.floor(rect.height);

    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = width * dpr;
    this.canvas.height = height * dpr;
    this.canvas.style.width = `${width}px`;
    this.canvas.style.height = `${height}px`;

    this.ctx.setTransform(1, 0, 0, 1, 0, 0);
    this.ctx.scale(dpr, dpr);

    this.createPixels();
  }

  getDistanceToCenter(x: number, y: number) {
    const dx = x - this.canvas.width / 2;
    const dy = y - this.canvas.height / 2;
    return Math.sqrt(dx * dx + dy * dy);
  }

  getDistanceToBottomLeft(x: number, y: number) {
    const dx = x;
    const dy = this.canvas.height - y;
    return Math.sqrt(dx * dx + dy * dy);
  }

  resolveColor(color: string) {
    if (typeof window !== "undefined" && color.startsWith("var(")) {
      const varName = color.replace(/^var\((--[\w-]+)\)$/, "$1").trim();
      return getComputedStyle(document.documentElement).getPropertyValue(varName).trim() || color;
    }
    return color;
  }

  createPixels() {
    if (!this.ctx) return;
    this.pixels = [];

    for (let x = 0; x < this.canvas.width; x += this.gap) {
      for (let y = 0; y < this.canvas.height; y += this.gap) {
        const baseColor =
          this.colors[Math.floor(Math.random() * this.colors.length)] || "#ffffff";
          
        const color = this.resolveColor(baseColor);

        this.pixels.push(
          new Pixel(this.canvas, this.ctx, x, y, color, this.speed, 0)
        );
      }
    }
  }

  handleAnimation(name: "appear" | "disappear") {
    if (this.animation) {
      cancelAnimationFrame(this.animation);
    }

    const animate = () => {
      this.animation = requestAnimationFrame(animate);

      const timeNow = performance.now();
      const timePassed = timeNow - this.timePrevious;

      if (timePassed < this.timeInterval) return;

      this.timePrevious = timeNow - (timePassed % this.timeInterval);

      if (!this.ctx) return;
      this.ctx.clearRect(0, 0, this.canvas.width, this.canvas.height);

      let allIdle = true;
      for (const pixel of this.pixels) {
        pixel[name]();
        if (!pixel.isIdle) allIdle = false;
      }

      if (allIdle) {
        cancelAnimationFrame(this.animation);
        this.animation = null;
      }
    };

    animate();
  }
}

import * as React from "react";

export interface PixelCanvasProps extends React.HTMLAttributes<HTMLDivElement> {
  gap?: number;
  speed?: number;
  colors?: string[];
  variant?: "default" | "icon";
  noFocus?: boolean;
}

const PixelCanvas = React.forwardRef<HTMLDivElement, PixelCanvasProps>(({
    gap = 6,
    speed = 10,
    colors = [
      "var(--pixel-brand-light)",
      "var(--pixel-brand-sand)",
      "var(--pixel-brand-orange-fade)",
      "var(--pixel-brand-orange)",
      "var(--pixel-brand-red)",
      "var(--pixel-brand-maroon)",
    ],
    variant,
    noFocus,
    style,
    ...props
  }, ref) => {
    React.useEffect(() => {
      if (typeof window !== "undefined") {
        if (!customElements.get("pixel-canvas")) {
          customElements.define("pixel-canvas", PixelCanvasElement);
        }
      }
    }, []);

    const CustomTag = "pixel-canvas" as React.ElementType;

    return (
      <CustomTag
        ref={ref}
        data-gap={gap}
        data-speed={speed}
        data-colors={colors?.join(",")}
        data-variant={variant}
        {...(noFocus && { "data-no-focus": "" })}
        style={{
          position: "absolute",
          inset: 0,
          pointerEvents: "none",
          width: "100%",
          height: "100%",
          ...style,
        }}
        {...props}
      />
    );
  }
);
PixelCanvas.displayName = "PixelCanvas";

export { PixelCanvas };
