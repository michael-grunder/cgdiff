__attribute__((const)) /* <--- CONST ATTRIBUTE */
int add(int a, int b);

int add2(int a, int b) {
    return add(a, b) + add(a, b);
}
