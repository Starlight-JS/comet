let start = new Date();
let l = []
let i = 0;
while (i < 500000000) {
    l = [42, l];
    if (i % 8192 == 0) {
        l = [];
    }
    i += 1;
}
let end = new Date();

let diff = end - start;
diff /= 1000;
console.log('Elapsed: ' + diff);