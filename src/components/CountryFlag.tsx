import type { ReactNode } from "react";

interface CountryFlagProps {
  iso: string;
  name?: string;
  width?: number;
  height?: number;
}

function hashCode(s: string): number {
  let h = 2166136261;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return h >>> 0;
}

function renderFlagGraphics(code: string): ReactNode | null {
  switch (code) {
    case "ES":
      return (
        <>
          <rect width="24" height="16" fill="#AA151B" />
          <rect y="4" width="24" height="8" fill="#F1BF00" />
          <rect x="5.5" y="6" width="2.4" height="4" rx="0.6" fill="#AA151B" />
        </>
      );
    case "NL":
      return (
        <>
          <rect width="24" height="5.34" fill="#AE1C28" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#21468B" />
        </>
      );
    case "LU":
      return (
        <>
          <rect width="24" height="5.34" fill="#EF3340" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#00A3E0" />
        </>
      );
    case "FR":
      return (
        <>
          <rect width="8" height="16" fill="#002395" />
          <rect x="8" width="8" height="16" fill="#FFFFFF" />
          <rect x="16" width="8" height="16" fill="#ED2939" />
        </>
      );
    case "IT":
      return (
        <>
          <rect width="8" height="16" fill="#009246" />
          <rect x="8" width="8" height="16" fill="#FFFFFF" />
          <rect x="16" width="8" height="16" fill="#CE2B37" />
        </>
      );
    case "IE":
      return (
        <>
          <rect width="8" height="16" fill="#169B62" />
          <rect x="8" width="8" height="16" fill="#FFFFFF" />
          <rect x="16" width="8" height="16" fill="#FF883E" />
        </>
      );
    case "BE":
      return (
        <>
          <rect width="8" height="16" fill="#111111" />
          <rect x="8" width="8" height="16" fill="#FAE042" />
          <rect x="16" width="8" height="16" fill="#ED2939" />
        </>
      );
    case "RO":
      return (
        <>
          <rect width="8" height="16" fill="#002B7F" />
          <rect x="8" width="8" height="16" fill="#FCD116" />
          <rect x="16" width="8" height="16" fill="#CE1126" />
        </>
      );
    case "AD":
      return (
        <>
          <rect width="8" height="16" fill="#0018A8" />
          <rect x="8" width="8" height="16" fill="#FEDF00" />
          <rect x="16" width="8" height="16" fill="#D52B1E" />
          <circle cx="12" cy="8" r="1.6" fill="#8B5A2B" />
        </>
      );
    case "DE":
      return (
        <>
          <rect width="24" height="5.34" fill="#111111" />
          <rect y="5.33" width="24" height="5.34" fill="#DD0000" />
          <rect y="10.66" width="24" height="5.34" fill="#FFCE00" />
        </>
      );
    case "AT":
      return (
        <>
          <rect width="24" height="5.34" fill="#ED2939" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#ED2939" />
        </>
      );
    case "HU":
      return (
        <>
          <rect width="24" height="5.34" fill="#CD2A3E" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#436F4D" />
        </>
      );
    case "BG":
      return (
        <>
          <rect width="24" height="5.34" fill="#FFFFFF" />
          <rect y="5.33" width="24" height="5.34" fill="#00966E" />
          <rect y="10.66" width="24" height="5.34" fill="#D62612" />
        </>
      );
    case "EE":
      return (
        <>
          <rect width="24" height="5.34" fill="#0072CE" />
          <rect y="5.33" width="24" height="5.34" fill="#111111" />
          <rect y="10.66" width="24" height="5.34" fill="#FFFFFF" />
        </>
      );
    case "LV":
      return (
        <>
          <rect width="24" height="16" fill="#9E3039" />
          <rect y="6.4" width="24" height="3.2" fill="#FFFFFF" />
        </>
      );
    case "LT":
      return (
        <>
          <rect width="24" height="5.34" fill="#FDB913" />
          <rect y="5.33" width="24" height="5.34" fill="#006A44" />
          <rect y="10.66" width="24" height="5.34" fill="#C1272D" />
        </>
      );
    case "PL":
      return (
        <>
          <rect width="24" height="8" fill="#FFFFFF" />
          <rect y="8" width="24" height="8" fill="#DC143C" />
        </>
      );
    case "MC":
      return (
        <>
          <rect width="24" height="8" fill="#CE1126" />
          <rect y="8" width="24" height="8" fill="#FFFFFF" />
        </>
      );
    case "ID":
      return (
        <>
          <rect width="24" height="8" fill="#CE1126" />
          <rect y="8" width="24" height="8" fill="#FFFFFF" />
        </>
      );
    case "UA":
      return (
        <>
          <rect width="24" height="8" fill="#005BBB" />
          <rect y="8" width="24" height="8" fill="#FFD500" />
        </>
      );
    case "PT":
      return (
        <>
          <rect width="24" height="16" fill="#FF0000" />
          <rect width="9.5" height="16" fill="#006600" />
          <circle cx="9.5" cy="8" r="2.7" fill="#FFCC00" />
        </>
      );
    case "CH":
      return (
        <>
          <rect width="24" height="16" fill="#D52B1E" />
          <rect x="10.2" y="3.2" width="3.6" height="9.6" fill="#FFFFFF" />
          <rect x="7.2" y="6.2" width="9.6" height="3.6" fill="#FFFFFF" />
        </>
      );
    case "GB":
      return (
        <>
          <rect width="24" height="16" fill="#012169" />
          <path d="M0,0 L24,16 M24,0 L0,16" stroke="#FFFFFF" strokeWidth="3.2" />
          <path d="M0,0 L24,16 M24,0 L0,16" stroke="#C8102E" strokeWidth="1.6" />
          <rect x="9.6" width="4.8" height="16" fill="#FFFFFF" />
          <rect y="5.6" width="24" height="4.8" fill="#FFFFFF" />
          <rect x="10.5" width="3" height="16" fill="#C8102E" />
          <rect y="6.5" width="24" height="3" fill="#C8102E" />
        </>
      );
    case "US":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          {[0, 2, 4, 6, 8, 10, 12].map((i) => (
            <rect key={i} y={i * 1.23} width="24" height="1.23" fill="#B22234" />
          ))}
          <rect width="10.5" height="8.6" fill="#3C3B6E" />
          <circle cx="3" cy="2.5" r="0.7" fill="#FFFFFF" />
          <circle cx="5.5" cy="2.5" r="0.7" fill="#FFFFFF" />
          <circle cx="8" cy="2.5" r="0.7" fill="#FFFFFF" />
          <circle cx="4.2" cy="4.4" r="0.7" fill="#FFFFFF" />
          <circle cx="6.8" cy="4.4" r="0.7" fill="#FFFFFF" />
          <circle cx="3" cy="6.3" r="0.7" fill="#FFFFFF" />
          <circle cx="5.5" cy="6.3" r="0.7" fill="#FFFFFF" />
          <circle cx="8" cy="6.3" r="0.7" fill="#FFFFFF" />
        </>
      );
    case "CA":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <rect width="6" height="16" fill="#FF0000" />
          <rect x="18" width="6" height="16" fill="#FF0000" />
          <path d="M12,3.5 L13.4,6.4 L15.5,6 L14.2,8.5 L15.8,9.8 L12.6,10.1 L12.6,12.5 L11.4,12.5 L11.4,10.1 L8.2,9.8 L9.8,8.5 L8.5,6 L10.6,6.4 Z" fill="#FF0000" />
        </>
      );
    case "MX":
      return (
        <>
          <rect width="8" height="16" fill="#006847" />
          <rect x="8" width="8" height="16" fill="#FFFFFF" />
          <rect x="16" width="8" height="16" fill="#CE1126" />
          <circle cx="12" cy="8" r="2" fill="#8C6239" />
        </>
      );
    case "BR":
      return (
        <>
          <rect width="24" height="16" fill="#009C3B" />
          <polygon points="12,2 21,8 12,14 3,8" fill="#FFDF00" />
          <circle cx="12" cy="8" r="3" fill="#002776" />
        </>
      );
    case "AR":
      return (
        <>
          <rect width="24" height="5.34" fill="#74ACDF" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#74ACDF" />
          <circle cx="12" cy="8" r="1.7" fill="#F6B40E" />
        </>
      );
    case "CL":
      return (
        <>
          <rect width="24" height="8" fill="#FFFFFF" />
          <rect y="8" width="24" height="8" fill="#D52B1E" />
          <rect width="9" height="8" fill="#0039A6" />
          <circle cx="4.5" cy="4" r="1.5" fill="#FFFFFF" />
        </>
      );
    case "CO":
      return (
        <>
          <rect width="24" height="8" fill="#FCD116" />
          <rect y="8" width="24" height="4" fill="#003893" />
          <rect y="12" width="24" height="4" fill="#CE1126" />
        </>
      );
    case "PE":
      return (
        <>
          <rect width="8" height="16" fill="#D91023" />
          <rect x="8" width="8" height="16" fill="#FFFFFF" />
          <rect x="16" width="8" height="16" fill="#D91023" />
        </>
      );
    case "SE":
      return (
        <>
          <rect width="24" height="16" fill="#006AA7" />
          <rect x="7" width="3" height="16" fill="#FECC00" />
          <rect y="6.5" width="24" height="3" fill="#FECC00" />
        </>
      );
    case "NO":
      return (
        <>
          <rect width="24" height="16" fill="#BA0C2F" />
          <rect x="6.5" width="4" height="16" fill="#FFFFFF" />
          <rect y="6" width="24" height="4" fill="#FFFFFF" />
          <rect x="7.5" width="2" height="16" fill="#00205B" />
          <rect y="7" width="24" height="2" fill="#00205B" />
        </>
      );
    case "DK":
      return (
        <>
          <rect width="24" height="16" fill="#C60C30" />
          <rect x="7" width="2.6" height="16" fill="#FFFFFF" />
          <rect y="6.7" width="24" height="2.6" fill="#FFFFFF" />
        </>
      );
    case "FI":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <rect x="7" width="3.2" height="16" fill="#003580" />
          <rect y="6.4" width="24" height="3.2" fill="#003580" />
        </>
      );
    case "IS":
      return (
        <>
          <rect width="24" height="16" fill="#02529C" />
          <rect x="6.5" width="4" height="16" fill="#FFFFFF" />
          <rect y="6" width="24" height="4" fill="#FFFFFF" />
          <rect x="7.5" width="2" height="16" fill="#DC1E35" />
          <rect y="7" width="24" height="2" fill="#DC1E35" />
        </>
      );
    case "CZ":
      return (
        <>
          <rect width="24" height="8" fill="#FFFFFF" />
          <rect y="8" width="24" height="8" fill="#D7141A" />
          <polygon points="0,0 11,8 0,16" fill="#11457E" />
        </>
      );
    case "SK":
      return (
        <>
          <rect width="24" height="5.34" fill="#FFFFFF" />
          <rect y="5.33" width="24" height="5.34" fill="#0B4EA2" />
          <rect y="10.66" width="24" height="5.34" fill="#EE1C25" />
          <rect x="4.5" y="4.5" width="4" height="6" rx="1.5" fill="#EE1C25" stroke="#FFFFFF" strokeWidth="0.6" />
        </>
      );
    case "SI":
      return (
        <>
          <rect width="24" height="5.34" fill="#FFFFFF" />
          <rect y="5.33" width="24" height="5.34" fill="#005DA4" />
          <rect y="10.66" width="24" height="5.34" fill="#ED1C24" />
        </>
      );
    case "HR":
      return (
        <>
          <rect width="24" height="5.34" fill="#FF0000" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#171796" />
          <rect x="10" y="5.2" width="4" height="5" rx="0.8" fill="#FF0000" />
        </>
      );
    case "GR":
      return (
        <>
          <rect width="24" height="16" fill="#0D5EAF" />
          {[1.78, 5.33, 8.88, 12.44].map((y) => (
            <rect key={y} y={y} width="24" height="1.78" fill="#FFFFFF" />
          ))}
          <rect width="9" height="8.9" fill="#0D5EAF" />
          <rect x="3.6" width="1.8" height="8.9" fill="#FFFFFF" />
          <rect y="3.55" width="9" height="1.8" fill="#FFFFFF" />
        </>
      );
    case "TR":
      return (
        <>
          <rect width="24" height="16" fill="#E30A17" />
          <circle cx="9.6" cy="8" r="3.4" fill="#FFFFFF" />
          <circle cx="10.6" cy="8" r="2.7" fill="#E30A17" />
          <circle cx="13.5" cy="8" r="1.1" fill="#FFFFFF" />
        </>
      );
    case "JP":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <circle cx="12" cy="8" r="4.2" fill="#BC002D" />
        </>
      );
    case "CN":
      return (
        <>
          <rect width="24" height="16" fill="#DE2910" />
          <circle cx="5.2" cy="4.8" r="1.9" fill="#FFDE00" />
          <circle cx="8.8" cy="2.7" r="0.6" fill="#FFDE00" />
          <circle cx="9.8" cy="4.6" r="0.6" fill="#FFDE00" />
          <circle cx="9.2" cy="6.6" r="0.6" fill="#FFDE00" />
        </>
      );
    case "KR":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <path d="M12,4 A4,4 0 0,1 12,12 A2,2 0 0,1 12,8 A2,2 0 0,0 12,4 Z" fill="#CD2E3A" />
          <path d="M12,4 A2,2 0 0,0 12,8 A2,2 0 0,1 12,12 A4,4 0 0,1 12,4 Z" fill="#0047A0" />
        </>
      );
    case "TW":
      return (
        <>
          <rect width="24" height="16" fill="#FE0000" />
          <rect width="12" height="8" fill="#000095" />
          <circle cx="6" cy="4" r="2.1" fill="#FFFFFF" />
        </>
      );
    case "AU":
      return (
        <>
          <rect width="24" height="16" fill="#00008B" />
          <rect width="10.5" height="7" fill="#012169" />
          <path d="M0,0 L10.5,7 M10.5,0 L0,7" stroke="#FFFFFF" strokeWidth="1.4" />
          <rect x="4.3" width="1.9" height="7" fill="#FFFFFF" />
          <rect y="2.6" width="10.5" height="1.8" fill="#FFFFFF" />
          <rect x="4.7" width="1.1" height="7" fill="#C8102E" />
          <rect y="3" width="10.5" height="1" fill="#C8102E" />
          <circle cx="5.2" cy="11.8" r="1.3" fill="#FFFFFF" />
          <circle cx="17.5" cy="4.2" r="0.8" fill="#FFFFFF" />
          <circle cx="19.5" cy="7.6" r="0.8" fill="#FFFFFF" />
          <circle cx="16.5" cy="9.4" r="0.8" fill="#FFFFFF" />
          <circle cx="18.3" cy="12.2" r="0.8" fill="#FFFFFF" />
        </>
      );
    case "NZ":
      return (
        <>
          <rect width="24" height="16" fill="#00247D" />
          <rect width="10.5" height="7" fill="#012169" />
          <path d="M0,0 L10.5,7 M10.5,0 L0,7" stroke="#FFFFFF" strokeWidth="1.4" />
          <rect x="4.3" width="1.9" height="7" fill="#FFFFFF" />
          <rect y="2.6" width="10.5" height="1.8" fill="#FFFFFF" />
          <rect x="4.7" width="1.1" height="7" fill="#C8102E" />
          <rect y="3" width="10.5" height="1" fill="#C8102E" />
          <circle cx="17.5" cy="4.2" r="0.9" fill="#CC142B" stroke="#FFFFFF" strokeWidth="0.35" />
          <circle cx="19.8" cy="7.2" r="0.9" fill="#CC142B" stroke="#FFFFFF" strokeWidth="0.35" />
          <circle cx="15.8" cy="7.5" r="0.9" fill="#CC142B" stroke="#FFFFFF" strokeWidth="0.35" />
          <circle cx="17.8" cy="11.5" r="0.9" fill="#CC142B" stroke="#FFFFFF" strokeWidth="0.35" />
        </>
      );
    case "ZA":
      return (
        <>
          <rect width="24" height="8" fill="#E03C31" />
          <rect y="8" width="24" height="8" fill="#001489" />
          <path d="M0,1 L9,8 L0,15" fill="none" stroke="#FFB81C" strokeWidth="2.8" />
          <path d="M0,0 L10,8 L0,16 M10,8 L24,8" fill="none" stroke="#007749" strokeWidth="3.2" />
          <polygon points="0,3 6.5,8 0,13" fill="#111111" />
        </>
      );
    case "AE":
      return (
        <>
          <rect width="24" height="5.34" fill="#00732F" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#111111" />
          <rect width="6.5" height="16" fill="#FF0000" />
        </>
      );
    case "SG":
      return (
        <>
          <rect width="24" height="8" fill="#EF3340" />
          <rect y="8" width="24" height="8" fill="#FFFFFF" />
          <circle cx="5.2" cy="4" r="2.2" fill="#FFFFFF" />
          <circle cx="6.2" cy="4" r="1.8" fill="#EF3340" />
        </>
      );
    case "TH":
      return (
        <>
          <rect width="24" height="16" fill="#A51931" />
          <rect y="2.6" width="24" height="10.8" fill="#FFFFFF" />
          <rect y="5.3" width="24" height="5.4" fill="#2D2A4A" />
        </>
      );
    case "IN":
      return (
        <>
          <rect width="24" height="5.34" fill="#FF9933" />
          <rect y="5.33" width="24" height="5.34" fill="#FFFFFF" />
          <rect y="10.66" width="24" height="5.34" fill="#138808" />
          <circle cx="12" cy="8" r="2" fill="none" stroke="#000080" strokeWidth="0.9" />
        </>
      );
    case "MA":
      return (
        <>
          <rect width="24" height="16" fill="#C1272D" />
          <polygon points="12,4.5 13.8,10.5 8.8,6.9 15.2,6.9 10.2,10.5" fill="none" stroke="#006233" strokeWidth="1.1" />
        </>
      );
    case "MT":
      return (
        <>
          <rect width="12" height="16" fill="#FFFFFF" />
          <rect x="12" width="12" height="16" fill="#CF142B" />
          <rect x="2.4" y="2.4" width="2.4" height="2.4" fill="#A0A0A0" />
        </>
      );
    case "CY":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <path d="M7,9.5 L12,6.5 L16.5,7.2 L14,10.5 Z" fill="#D57800" />
          <path d="M8.5,12.2 Q12,13.6 15.5,12.2" fill="none" stroke="#4E5B31" strokeWidth="1" />
        </>
      );
    case "IL":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          <rect y="1.8" width="24" height="2.4" fill="#0038B8" />
          <rect y="11.8" width="24" height="2.4" fill="#0038B8" />
          <polygon points="12,5 14.6,9.5 9.4,9.5" fill="none" stroke="#0038B8" strokeWidth="0.9" />
          <polygon points="12,11 14.6,6.5 9.4,6.5" fill="none" stroke="#0038B8" strokeWidth="0.9" />
        </>
      );
    case "HK":
      return (
        <>
          <rect width="24" height="16" fill="#DE2910" />
          <circle cx="12" cy="8" r="3" fill="#FFFFFF" />
          <circle cx="12" cy="8" r="1" fill="#DE2910" />
        </>
      );
    case "MY":
      return (
        <>
          <rect width="24" height="16" fill="#FFFFFF" />
          {[0, 2, 4, 6, 8, 10, 12].map((i) => (
            <rect key={i} y={i * 1.23} width="24" height="1.23" fill="#CC0001" />
          ))}
          <rect width="12" height="8.6" fill="#010066" />
          <circle cx="5.2" cy="4.3" r="2.1" fill="#FFCC00" />
          <circle cx="6.2" cy="4.3" r="1.7" fill="#010066" />
        </>
      );
    case "VN":
      return (
        <>
          <rect width="24" height="16" fill="#DA251D" />
          <polygon points="12,4.2 13.1,7.2 16.3,7.2 13.7,9.1 14.7,12.1 12,10.2 9.3,12.1 10.3,9.1 7.7,7.2 10.9,7.2" fill="#FFFF00" />
        </>
      );
    case "PH":
      return (
        <>
          <rect width="24" height="8" fill="#0038A8" />
          <rect y="8" width="24" height="8" fill="#CE1126" />
          <polygon points="0,0 10.5,8 0,16" fill="#FFFFFF" />
          <circle cx="4" cy="8" r="1.7" fill="#FCD116" />
        </>
      );
    default:
      return null;
  }
}

export function CountryFlag({ iso, name, width = 20, height = 14 }: CountryFlagProps) {
  const code = (iso || "").toUpperCase();
  const known = renderFlagGraphics(code);
  const label = code && code !== "-99" ? code.slice(0, 2) : (name ?? "?").slice(0, 2).toUpperCase();
  const palette = ["#2563EB", "#0D9488", "#D97706", "#7C3AED", "#DB2777", "#2D6A4F"];
  const bg = palette[hashCode(label) % palette.length];

  return (
    <svg
      viewBox="0 0 24 16"
      width={width}
      height={height}
      role="img"
      aria-label={name ? `${name} flag` : `${label} flag`}
      style={{
        display: "inline-block",
        verticalAlign: "middle",
        borderRadius: 3,
        boxShadow: "0 0 0 1px rgba(255,255,255,0.16)",
        flexShrink: 0,
      }}
    >
      {known ? (
        known
      ) : (
        <>
          <rect width="24" height="16" fill={bg} />
          <text
            x="12"
            y="11"
            textAnchor="middle"
            fontSize="8.5"
            fontWeight="700"
            fill="#FFFFFF"
            fontFamily="system-ui, -apple-system, sans-serif"
          >
            {label}
          </text>
        </>
      )}
    </svg>
  );
}
