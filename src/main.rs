use crdts::{list::Op, CmRDT, List};

fn main() {
    let mut text = List::new();

    "Hello world".chars().for_each(|c| {
        text.apply(text.append(c, 1))
    });

    (6..11).rev().for_each(|i| {
        text.apply(text.delete_index(i, 1).unwrap())
    });
    "mom".chars().for_each(|c| text.apply(text.append(c, 1)));

    text.apply(text.delete_index(4, 2).unwrap());

    let s: String = text.read();
    println!("{:?}", s);
}
