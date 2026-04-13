import { useEffect, useRef, useState } from "react";
import type { MouseEvent } from "react";

const identityMatrix = "1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1";
const maxRotate = 0.25;
const minRotate = -0.25;
const maxScale = 1;
const minScale = 0.97;

// Original holographic colors for the overlay
const colors = [
  "hsl(358, 100%, 62%)", "hsl(30, 100%, 50%)", "hsl(60, 100%, 50%)", 
  "hsl(96, 100%, 50%)", "hsl(233, 85%, 47%)", "hsl(271, 85%, 47%)", 
  "hsl(300, 20%, 35%)", "transparent", "transparent", "white"
];

interface LogoStickerProps {
  className?: string;
  iconSrc?: string;
}

export const LogoSticker = ({ 
  className = "", 
  iconSrc = "/icon.svg",
}: LogoStickerProps) => {
  const ref = useRef<HTMLDivElement>(null);
  const [firstOverlayPosition, setFirstOverlayPosition] = useState<number>(0);
  const [matrix, setMatrix] = useState<string>(identityMatrix);
  const [currentMatrix, setCurrentMatrix] = useState<string>(identityMatrix);
  const [isTimeoutFinished, setIsTimeoutFinished] = useState(true);
  const [disableInOutOverlayAnimation, setDisableInOutOverlayAnimation] = useState(true);
  const [disableOverlayAnimation, setDisableOverlayAnimation] = useState(false);

  const enterTimeoutRef = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const leaveTimeoutRef1 = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const leaveTimeoutRef2 = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const leaveTimeoutRef3 = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);

  useEffect(() => {
    let animationFrameId: number;
    const animate = () => {
      if (isTimeoutFinished) {
        setMatrix(currentMatrix);
      }
      animationFrameId = requestAnimationFrame(animate);
    };
    animate();
    return () => cancelAnimationFrame(animationFrameId);
  }, [currentMatrix, isTimeoutFinished]);

  const getOppositeMatrix = (matrixStr: string, clientY: number, onMouseEnter = false) => {
    if (!ref.current) return matrixStr;
    const rect = ref.current.getBoundingClientRect();
    const oppositeY = rect.bottom - clientY + rect.top;
    const weakening = onMouseEnter ? 0.7 : 4;
    const multiplier = onMouseEnter ? -1 : 1;

    return matrixStr.split(", ").map((item, index) => {
      if (index === 2 || index === 4 || index === 8) {
        return -parseFloat(item) * multiplier / weakening;
      } else if (index === 0 || index === 5 || index === 10) {
        return "1";
      } else if (index === 6 || index === 9) {
        const sign = index === 6 ? multiplier : 1;
        return sign * (maxRotate - (maxRotate - minRotate) * (rect.top - oppositeY) / (rect.top - rect.bottom)) / weakening;
      }
      return item;
    }).join(", ");
  };

  const getMatrix = (clientX: number, clientY: number) => {
    if (!ref.current) return identityMatrix;
    const rect = ref.current.getBoundingClientRect();
    const xCenter = (rect.left + rect.right) / 2;
    const yCenter = (rect.top + rect.bottom) / 2;

    const scale = [
      maxScale - (maxScale - minScale) * Math.abs(xCenter - clientX) / (xCenter - rect.left),
      maxScale - (maxScale - minScale) * Math.abs(yCenter - clientY) / (yCenter - rect.top),
      maxScale - (maxScale - minScale) * (Math.abs(xCenter - clientX) + Math.abs(yCenter - clientY)) / (xCenter - rect.left + yCenter - rect.top)
    ];

    const rotate = {
      x1: 0.25 * ((yCenter - clientY) / yCenter - (xCenter - clientX) / xCenter),
      x2: maxRotate - (maxRotate - minRotate) * Math.abs(rect.right - clientX) / (rect.right - rect.left),
      y2: maxRotate - (maxRotate - minRotate) * (rect.top - clientY) / (rect.top - rect.bottom),
      z0: -(maxRotate - (maxRotate - minRotate) * Math.abs(rect.right - clientX) / (rect.right - rect.left)),
      z1: (0.2 - (0.2 + 0.6) * (rect.top - clientY) / (rect.top - rect.bottom)),
    };

    return `${scale[0]}, 0, ${rotate.z0}, 0, ` +
           `${rotate.x1}, ${scale[1]}, ${rotate.z1}, 0, ` +
           `${rotate.x2}, ${rotate.y2}, ${scale[2]}, 0, ` +
           `0, 0, 0, 1`;
  };

  const onMouseEnter = (e: MouseEvent<HTMLDivElement>) => {
    clearTimeout(leaveTimeoutRef1.current);
    clearTimeout(leaveTimeoutRef2.current);
    clearTimeout(leaveTimeoutRef3.current);
    
    setDisableOverlayAnimation(true);
    setDisableInOutOverlayAnimation(false);

    enterTimeoutRef.current = setTimeout(() => {
      setDisableInOutOverlayAnimation(true);
    }, 350);

    if (ref.current) {
      const rect = ref.current.getBoundingClientRect();
      const xCenter = (rect.left + rect.right) / 2;
      const yCenter = (rect.top + rect.bottom) / 2;
      
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          setFirstOverlayPosition((Math.abs(xCenter - e.clientX) + Math.abs(yCenter - e.clientY)) / 1.5);
        });
      });
    }

    const newMatrix = getMatrix(e.clientX, e.clientY);
    const oppositeMatrix = getOppositeMatrix(newMatrix, e.clientY, true);
    
    setMatrix(oppositeMatrix);
    setIsTimeoutFinished(false);
    setTimeout(() => setIsTimeoutFinished(true), 200);
  };

  const onMouseMove = (e: MouseEvent<HTMLDivElement>) => {
    if (ref.current) {
      const rect = ref.current.getBoundingClientRect();
      const xCenter = (rect.left + rect.right) / 2;
      const yCenter = (rect.top + rect.bottom) / 2;
      
      setTimeout(() => {
        setFirstOverlayPosition((Math.abs(xCenter - e.clientX) + Math.abs(yCenter - e.clientY)) / 1.5);
      }, 150);
    }

    if (isTimeoutFinished) {
      setCurrentMatrix(getMatrix(e.clientX, e.clientY));
    }
  };

  const onMouseLeave = (e: MouseEvent<HTMLDivElement>) => {
    clearTimeout(enterTimeoutRef.current);
    
    const oppositeMatrix = getOppositeMatrix(matrix, e.clientY);
    setCurrentMatrix(oppositeMatrix);
    setTimeout(() => setCurrentMatrix(identityMatrix), 200);

    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        setDisableInOutOverlayAnimation(false);
        
        leaveTimeoutRef1.current = setTimeout(() => {
          setFirstOverlayPosition((prev: number) => -prev / 4);
        }, 150);
        
        leaveTimeoutRef2.current = setTimeout(() => {
          setFirstOverlayPosition(0);
        }, 300);
        
        leaveTimeoutRef3.current = setTimeout(() => {
          setDisableOverlayAnimation(false);
          setDisableInOutOverlayAnimation(true);
        }, 500);
      });
    });
  };

  return (
    <div 
      className={`logo-sticker ${className}`} 
      onMouseEnter={onMouseEnter} 
      onMouseMove={onMouseMove} 
      onMouseLeave={onMouseLeave}
      style={{
        display: "inline-block",
        perspective: "700px",
        cursor: "pointer",
        width: 104, // Default bounds matching CSS
        height: 104,
      }}
    >
      <div 
        ref={ref}
        style={{ 
          transform: `matrix3d(${matrix})`,
          width: "100%",
          height: "100%",
          position: "relative",
          transformOrigin: "center center",
          transition: "transform 200ms ease-out",
          filter: "drop-shadow(0 20px 30px rgba(216, 92, 49, 0.4))",
        }} 
      >
        {/* Base Original Logo */}
        <img 
          src={iconSrc} 
          alt="LAD Logo" 
          style={{
            width: "100%",
            height: "100%",
            position: "absolute",
            inset: 0,
            objectFit: "contain",
          }}
        />

        {/* Holographic Overlay Layer exactly masked to the logo's alpha shape */}
        <div
          style={{
            position: "absolute",
            inset: 0,
            width: "100%",
            height: "100%",
            WebkitMaskImage: `url(${iconSrc})`,
            WebkitMaskSize: "contain",
            WebkitMaskRepeat: "no-repeat",
            WebkitMaskPosition: "center",
            maskImage: `url(${iconSrc})`,
            maskSize: "contain",
            maskRepeat: "no-repeat",
            maskPosition: "center",
            mixBlendMode: "color-dodge", // "color-dodge" or "overlay" are best for shine over original colors
            pointerEvents: "none",
            zIndex: 1,
            opacity: 0.85
          }}
        >
          <svg 
            xmlns="http://www.w3.org/2000/svg" 
            viewBox="-50 -50 200 200" 
            style={{ 
              width: "100%", 
              height: "100%",
              overflow: "visible"
            }}
          >
            <defs>
              <filter id="blur1">
                <feGaussianBlur in="SourceGraphic" stdDeviation="8" />
              </filter>
            </defs>
            <g>
              {Array.from({ length: 10 }).map((_, i) => (
                <g
                  key={i}
                  style={{
                    transform: `rotate(${firstOverlayPosition + i * 10}deg)`,
                    transformOrigin: "50% 50%",
                    transition: !disableInOutOverlayAnimation ? "transform 200ms ease-out" : "none",
                    animation: !disableOverlayAnimation ? `overlayAnimation${i + 1} 5s infinite` : "none",
                    willChange: "transform"
                  }}
                >
                  {/* Narrow hourglass polygons (sunburst rays) intersecting at center */}
                  <polygon points="0,40 100,45 100,55 0,60" fill={colors[i]} filter="url(#blur1)" opacity="0.6" />
                </g>
              ))}
            </g>
          </svg>
        </div>
      </div>

      <style>{`
        ${Array.from({ length: 10 }).map((_, i) => `
          @keyframes overlayAnimation${i + 1} {
            0% { transform: rotate(${i * 10}deg); }
            50% { transform: rotate(${(i + 1) * 10}deg); }
            100% { transform: rotate(${i * 10}deg); }
          }
        `).join('')}
      `}</style>
    </div>
  );
};
