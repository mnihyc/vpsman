export function normalizeCountryCode(
  country: string | null | undefined,
): string | null {
  const normalized = country?.trim().toUpperCase() ?? "";
  return normalized || null;
}

export function CountryFlag({
  country,
  decorative = false,
  fallback = "code",
}: {
  country: string | null | undefined;
  decorative?: boolean;
  fallback?: "code" | "none";
}) {
  const normalized = normalizeCountryCode(country);
  if (!normalized) return null;
  const flag = unicodeCountryFlag(normalized);
  if (!flag) {
    return fallback === "code" ? (
      <span className="countryFlag countryFlagCode" title={normalized}>
        {normalized}
      </span>
    ) : null;
  }
  return (
    <span
      aria-label={decorative ? undefined : `${normalized} flag`}
      aria-hidden={decorative || undefined}
      className="countryFlag countryFlagGlyph"
      role={decorative ? undefined : "img"}
      title={decorative ? undefined : `${normalized} flag`}
    >
      {flag}
    </span>
  );
}

function unicodeCountryFlag(country: string): string | null {
  if (!ISO_ALPHA_2_COUNTRY_CODES.has(country)) return null;
  return Array.from(country, (letter) =>
    String.fromCodePoint(0x1f1e6 + letter.charCodeAt(0) - 65),
  ).join("");
}

// ISO 3166-1 alpha-2 assigned country and territory codes. Keeping the complete
// set here prevents arbitrary two-letter tags from being presented as countries.
const ISO_ALPHA_2_COUNTRY_CODES = new Set(
  `AD AE AF AG AI AL AM AO AQ AR AS AT AU AW AX AZ
BA BB BD BE BF BG BH BI BJ BL BM BN BO BQ BR BS BT BV BW BY BZ
CA CC CD CF CG CH CI CK CL CM CN CO CR CU CV CW CX CY CZ
DE DJ DK DM DO DZ EC EE EG EH ER ES ET FI FJ FK FM FO FR
GA GB GD GE GF GG GH GI GL GM GN GP GQ GR GS GT GU GW GY
HK HM HN HR HT HU ID IE IL IM IN IO IQ IR IS IT JE JM JO JP
KE KG KH KI KM KN KP KR KW KY KZ LA LB LC LI LK LR LS LT LU LV LY
MA MC MD ME MF MG MH MK ML MM MN MO MP MQ MR MS MT MU MV MW MX MY MZ
NA NC NE NF NG NI NL NO NP NR NU NZ OM PA PE PF PG PH PK PL PM PN PR PS PT PW PY
QA RE RO RS RU RW SA SB SC SD SE SG SH SI SJ SK SL SM SN SO SR SS ST SV SX SY SZ
TC TD TF TG TH TJ TK TL TM TN TO TR TT TV TW TZ UA UG UM US UY UZ
VA VC VE VG VI VN VU WF WS YE YT ZA ZM ZW`.split(/\s+/),
);

export function CountryBadge({
  country,
  showFlag,
}: {
  country: string | null | undefined;
  showFlag: boolean;
}) {
  const normalized = normalizeCountryCode(country);
  if (!normalized) return <span className="countryBadge">unset</span>;
  return (
    <span className="countryBadge" title={normalized}>
      {showFlag ? (
        <CountryFlag country={normalized} decorative fallback="none" />
      ) : null}
      <span>{normalized}</span>
    </span>
  );
}
