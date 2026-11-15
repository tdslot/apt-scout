//! ISO 3166 alpha-2 code to the country name Launchpad uses (82 countries on its mirror page).

pub fn launchpad_name(cc: &str) -> Option<&'static str> {
    TABLE.iter().find(|(c, _)| *c == cc).map(|(_, n)| *n)
}

const TABLE: [(&str, &str); 82] = [
    ("DZ", "Algeria"),
    ("AR", "Argentina"),
    ("AM", "Armenia"),
    ("AU", "Australia"),
    ("AT", "Austria"),
    ("AZ", "Azerbaijan"),
    ("BD", "Bangladesh"),
    ("BY", "Belarus"),
    ("BE", "Belgium"),
    ("BA", "Bosnia and Herzegovina"),
    ("BR", "Brazil"),
    ("BG", "Bulgaria"),
    ("KH", "Cambodia"),
    ("CA", "Canada"),
    ("CL", "Chile"),
    ("CN", "China"),
    ("CO", "Colombia"),
    ("CR", "Costa Rica"),
    ("HR", "Croatia"),
    ("CZ", "Czech Republic"),
    ("DK", "Denmark"),
    ("EC", "Ecuador"),
    ("EE", "Estonia"),
    ("FI", "Finland"),
    ("FR", "France"),
    ("GE", "Georgia"),
    ("DE", "Germany"),
    ("GR", "Greece"),
    ("GL", "Greenland"),
    ("HK", "Hong Kong"),
    ("HU", "Hungary"),
    ("IS", "Iceland"),
    ("IN", "India"),
    ("ID", "Indonesia"),
    ("IR", "Iran, Islamic Republic of"),
    ("IE", "Ireland"),
    ("IL", "Israel"),
    ("IT", "Italy"),
    ("JP", "Japan"),
    ("KZ", "Kazakhstan"),
    ("KE", "Kenya"),
    ("KR", "Korea, Republic of"),
    ("KG", "Kyrgyzstan"),
    ("LV", "Latvia"),
    ("LT", "Lithuania"),
    ("LU", "Luxembourg"),
    ("MK", "Macedonia, Republic of"),
    ("MY", "Malaysia"),
    ("MD", "Moldova, Republic of"),
    ("MN", "Mongolia"),
    ("MA", "Morocco"),
    ("NL", "Netherlands"),
    ("NC", "New Caledonia"),
    ("NZ", "New Zealand"),
    ("NO", "Norway"),
    ("PH", "Philippines"),
    ("PL", "Poland"),
    ("PT", "Portugal"),
    ("PR", "Puerto Rico"),
    ("RO", "Romania"),
    ("RU", "Russian Federation"),
    ("SA", "Saudi Arabia"),
    ("RS", "Serbia"),
    ("SG", "Singapore"),
    ("SK", "Slovakia"),
    ("SI", "Slovenia"),
    ("ZA", "South Africa"),
    ("ES", "Spain"),
    ("SE", "Sweden"),
    ("CH", "Switzerland"),
    ("TW", "Taiwan"),
    ("TZ", "Tanzania, United Republic of"),
    ("TH", "Thailand"),
    ("TN", "Tunisia"),
    ("TR", "Turkey"),
    ("UG", "Uganda"),
    ("UA", "Ukraine"),
    ("AE", "United Arab Emirates"),
    ("GB", "United Kingdom"),
    ("US", "United States"),
    ("UY", "Uruguay"),
    ("VN", "Viet Nam"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mirrors;

    #[test]
    fn known_codes() {
        assert_eq!(launchpad_name("LT"), Some("Lithuania"));
        assert_eq!(launchpad_name("GB"), Some("United Kingdom"));
        assert_eq!(launchpad_name("KR"), Some("Korea, Republic of"));
        assert_eq!(launchpad_name("XX"), None);
        assert_eq!(launchpad_name("lt"), None); // callers uppercase first
    }

    #[test]
    fn codes_are_unique_and_two_letters() {
        let mut seen = std::collections::HashSet::new();
        for (cc, _) in TABLE {
            assert_eq!(cc.len(), 2);
            assert!(seen.insert(cc), "duplicate {cc}");
        }
    }

    /// Every country on the full real Launchpad page must be in the table, and vice versa.
    #[test]
    fn matches_the_full_launchpad_page() {
        let html = include_str!("../tests/fixtures/launchpad-full-82-countries.html");
        let on_page: std::collections::HashSet<String> = mirrors::parse_launchpad(html)
            .expect("parses")
            .into_iter()
            .map(|m| m.country)
            .collect();
        let in_table: std::collections::HashSet<String> =
            TABLE.iter().map(|(_, n)| n.to_string()).collect();
        let missing: Vec<_> = on_page.difference(&in_table).collect();
        assert!(
            missing.is_empty(),
            "countries on Launchpad but not in the table: {missing:?}"
        );
        // a country can have only ftp/rsync mirrors and so not appear in parsed mirrors
        let extra: Vec<_> = in_table.difference(&on_page).collect();
        assert!(
            extra.len() <= 2,
            "table countries with no http(s) mirrors on the page: {extra:?}"
        );
    }
}
