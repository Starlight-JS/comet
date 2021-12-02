#include <stdio.h>
int main()
{
    {
        int d, i = 1;
        i++;
        d = ++i + 2;
        printf("d=%i\n", d);
    }
    {
        int d, i = 1;
        i++;
        d = ++i;
        printf("d=%i\n", d);
    }
    {
        int d = 1;
        d += ++d;
        printf("d=%i\n", d);
    }
    {
        int a, b, c, d, k;
        b = 2;
        d = 3;
        /*a = b;
        c = d;*/
        k = (a = b) + (c = d);
        printf("k=%i\na=%i\nc=%i\n", k, a, c);
    }

    {
        int i, l, j, k;
        i = l = j = k = 0;
        int a = i++ && ++j || k || l++;
        printf("a=%i\n", a);
    }
    {
        int a, b, k;
        a = 2;
        b = 1;
        k = (a != b) ? (a - b++) : (++a - b);
        printf("k=%i\n", k);
    }
    {
        int a = 2;
        int b = 3;
        float y1, y2;
        int c = 3.5;
        y1 = c * a / b;
        y2 = c * (a / b);
        printf("int c = %i\n", c);
        printf("float c = %f\n", c);
        printf("y1 = %f\n", y1);
        printf("y2 = %f\n", y2);
    }
}