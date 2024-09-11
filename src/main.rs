use std::{io::{self, Read, Write}, net::{TcpListener, TcpStream}};

use crdts::{list::Op, CmRDT, List};

fn main() {
    let mut stream: TcpStream;
    let args = std::env::args().collect::<Vec<String>>();
    let server = args.contains(&String::from("server"));
    if server {
        let listener = TcpListener::bind("0.0.0.0:6969");
        stream = listener.unwrap().accept().unwrap().0;
    } else {
        let mut address = String::new();
        io::stdin().read_line(&mut address).unwrap();
        stream = TcpStream::connect(address.trim()).unwrap();
    }

    if server {
        let mut s = String::new();
        stream.read_to_string(&mut s).unwrap();
        println!("{:?}", s);
    } else {
        write!(stream, "Hello world").unwrap();
    }
    
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
