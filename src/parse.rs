use std::error::Error;

use regex::Regex;

pub fn grep_issues(log: &String) -> Result<(), Box<dyn Error>> {
    // Matches messages of the following
    // filepath:lineno:linecol?: error: <message>\n
    //  <indented followups>
    // <make error>
    // https://www.gnu.org/prep/standards/html_node/Errors.html
    let re =
        Regex::new(r"(?m)^[a-zA-Z0-9_\-./ ]+:[0-9]+(:[0-9]+)?: error: .*(\n\s*.*)*?\nmake.*$")?;

    re.find_iter(log).for_each(|m| {
        println!("START MATCH--------");
        println!("{}", m.as_str());
        println!("END MATCH--------");
    });

    Ok(())
}
