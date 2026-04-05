import { useState } from 'react';
import { Copy, Check } from 'lucide-react';
import { LogoSticker } from './components/LogoSticker';

function App() {
  const [copied, setCopied] = useState(false);

  const handleCopy = () => {
    navigator.clipboard.writeText('npm serve lad');
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <>
      <div className="flare flare-orange"></div>
      <div className="flare flare-purple"></div>

      <nav className="navbar">
        <div className="brand">
          <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5"/>
          </svg>
          lad
        </div>
        <div className="nav-links">
          <a href="#">Documentation</a>
          <a href="#">GitHub</a>
        </div>
      </nav>

      <main className="hero">
        <LogoSticker className="hero-logo animate-in slide-up" />
        
        <h1 className="animate-in slide-up delay-1">
          AI agents shouldn't <br />
          <span className="gradient-text">read HTML.</span>
        </h1>
        
        <p className="hero-subtitle animate-in slide-up delay-2">
          Deliver lean, semantically rich, minimal DOM to your LLMs instantly. 
          Stop wasting tokens on layout wrappers, classes, and empty divs.
        </p>

        <div className="hero-actions animate-in fade-in delay-3">
          <a href="#get-started" className="btn btn-primary">Get Started</a>
          
          <div className="snippet">
            <code>npm serve lad</code>
            <button onClick={handleCopy} className="icon-copy" title="Copy to clipboard">
              {copied ? <Check size={18} /> : <Copy size={18} />}
            </button>
          </div>
        </div>

        <section className="showcase animate-in slide-up delay-4">
          <div className="glass-terminal">
            <div className="terminal-header">
              <div className="dot red"></div>
              <div className="dot yellow"></div>
              <div className="dot green"></div>
              <span className="tab">index.html</span>
              <span className="tab" style={{color: 'var(--accent-orange)'}}>lad.json</span>
            </div>
            
            <div className="terminal-body">
              <div className="pane pane-red">
                <span className="pane-title">Before (12KB)</span>
                <pre>{`<div class="flex-container">
  <div class="sidebar hidden-mobile">
    <nav aria-label="main">
      <ul class="nav-list">
        <li>
          <a href="/home" class="btn text-blue">
            <span>Home</span>
          </a>
        </li>
      </ul>
    </nav>
  </div>
</div>`}</pre>
              </div>
              
              <div className="pane pane-green">
                <span className="pane-title">After (450B)</span>
                <pre>{`{
  "role": "navigation",
  "links": [
    {
      "text": "Home",
      "href": "/home"
    }
  ]
}`}</pre>
              </div>
            </div>
          </div>
        </section>
      </main>

      <footer>
        <p>Built for the autonomous future by <a href="https://menot.sh" target="_blank" rel="noreferrer">menot-you</a>.</p>
      </footer>
    </>
  );
}

export default App;
