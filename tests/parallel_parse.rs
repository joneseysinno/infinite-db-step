//! Parallel STEP parse integration test.

use infinite_db_step::parse_step;

fn generate_step_with_vertices(n: usize) -> String {
    let mut body = String::new();
    for i in 1..=n {
        body.push_str(&format!(
            "#{} = CARTESIAN_POINT('',({}.0,0.0,0.0));\n",
            i, i
        ));
        body.push_str(&format!(
            "#{} = VERTEX_POINT('v{i}',#{i});\n",
            n + i,
            i = i
        ));
    }

    format!(
        r#"ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('Test'),'2;1');
FILE_NAME('test.stp','2024-01-01',(''),(''),'','','');
FILE_SCHEMA(('AUTOMOTIVE_DESIGN'));
ENDSEC;
DATA;
{body}ENDSEC;
END-ISO-10303-21;"#
    )
}

#[test]
fn parse_ten_thousand_vertices() {
    const N: usize = 10_000;
    let step = generate_step_with_vertices(N);
    let model = parse_step(&step).unwrap();
    assert_eq!(model.vertices.len(), N);
    assert_eq!(model.entities.len(), N * 2);
}
