import "./ssr-globals.js";
import createCache from "@emotion/cache";
import { CacheProvider } from "@emotion/react";
import createEmotionServer from "@emotion/server/create-instance";
import * as Icons from "@mui/icons-material";
import {
  AppBar,
  Avatar,
  Box,
  Button,
  Card,
  CardContent,
  Chip,
  CssBaseline,
  Drawer,
  List,
  ListItem,
  ListItemButton,
  ListItemText,
  Paper,
  Stack,
  Table,
  TableBody,
  TableCell,
  TableContainer,
  TableHead,
  TableRow,
  Toolbar,
  Typography,
} from "@mui/material";
import { ThemeProvider, createTheme } from "@mui/material/styles";
import { createElement } from "react";
import { renderToString } from "react-dom/server.browser";

const theme = createTheme();
const drawerWidth = 240;
const sections = ["Overview", "Orders", "Customers", "Products", "Invoices", "Reports", "Integrations", "Settings"];
const statuses = [
  ["Paid", "success"],
  ["Pending", "warning"],
  ["Refunded", "default"],
  ["Overdue", "error"],
];
const columns = ["Customer", "Email", "Status", "Total", "Actions"];
// Looking the icons up by name keeps every one of them in the bundle, as an
// application that renders icons chosen at runtime has them.
const shelf = Object.keys(Icons)
  .sort()
  .filter((_, index) => index % 36 === 0);

function customers(count) {
  return Array.from({ length: count }, (_, i) => ({
    id: i + 1,
    name: `Customer ${i + 1}`,
    email: `customer${i + 1}@example.com`,
    status: statuses[i % statuses.length],
    total: ((i * 7919) % 100000) / 100,
  }));
}

function Navigation() {
  return createElement(
    Drawer,
    {
      variant: "permanent",
      sx: { width: drawerWidth, flexShrink: 0, "& .MuiDrawer-paper": { width: drawerWidth, boxSizing: "border-box" } },
    },
    createElement(Toolbar),
    createElement(
      List,
      null,
      sections.map((section, index) =>
        createElement(
          ListItem,
          { key: section, disablePadding: true },
          createElement(ListItemButton, { selected: index === 0 }, createElement(ListItemText, { primary: section })),
        ),
      ),
    ),
  );
}

function Summary({ label, value }) {
  return createElement(
    Card,
    { variant: "outlined", sx: { flex: 1 } },
    createElement(
      CardContent,
      null,
      createElement(Typography, { color: "text.secondary", gutterBottom: true }, label),
      createElement(Typography, { variant: "h5", component: "div" }, value),
    ),
  );
}

function CustomerRow({ customer }) {
  const [label, color] = customer.status;
  return createElement(
    TableRow,
    { hover: true },
    createElement(
      TableCell,
      null,
      createElement(
        Stack,
        { direction: "row", spacing: 2, sx: { alignItems: "center" } },
        createElement(Avatar, { sx: { width: 32, height: 32 } }, `C${customer.id % 10}`),
        createElement(Typography, { variant: "body2" }, customer.name),
      ),
    ),
    createElement(TableCell, null, customer.email),
    createElement(TableCell, null, createElement(Chip, { label, color, size: "small" })),
    createElement(TableCell, { align: "right" }, customer.total.toFixed(2)),
    createElement(
      TableCell,
      { align: "right" },
      createElement(Button, { size: "small" }, "View"),
      createElement(Button, { size: "small", color: "error" }, "Refund"),
    ),
  );
}

function IconShelf() {
  return createElement(
    Box,
    { sx: { display: "grid", gridTemplateColumns: "repeat(auto-fill, minmax(140px, 1fr))", gap: 1, mt: 3 } },
    shelf.map((name) =>
      createElement(
        Stack,
        { key: name, sx: { alignItems: "center" } },
        createElement(Icons[name], { titleAccess: name }),
        createElement(Typography, { variant: "caption", noWrap: true }, name),
      ),
    ),
  );
}

function Dashboard({ rows }) {
  const list = customers(rows);
  const revenue = list.reduce((sum, customer) => sum + customer.total, 0);
  const count = (name) => list.filter((customer) => customer.status[0] === name).length;
  return createElement(
    Box,
    { sx: { display: "flex" } },
    createElement(CssBaseline),
    createElement(
      AppBar,
      { position: "fixed", sx: { zIndex: (current) => current.zIndex.drawer + 1 } },
      createElement(
        Toolbar,
        null,
        createElement(Typography, { variant: "h6", noWrap: true, component: "div", sx: { flexGrow: 1 } }, "Dashboard"),
        createElement(Button, { color: "inherit" }, "Sign out"),
      ),
    ),
    createElement(Navigation),
    createElement(
      Box,
      { component: "main", sx: { flexGrow: 1, p: 3 } },
      createElement(Toolbar),
      createElement(
        Stack,
        { direction: "row", spacing: 2, sx: { mb: 3 } },
        createElement(Summary, { label: "Customers", value: String(list.length) }),
        createElement(Summary, { label: "Revenue", value: revenue.toFixed(2) }),
        createElement(Summary, { label: "Pending", value: String(count("Pending")) }),
        createElement(Summary, { label: "Overdue", value: String(count("Overdue")) }),
      ),
      createElement(
        TableContainer,
        { component: Paper },
        createElement(
          Table,
          { size: "small" },
          createElement(
            TableHead,
            null,
            createElement(
              TableRow,
              null,
              columns.map((column, index) => createElement(TableCell, { key: column, align: index > 2 ? "right" : "left" }, column)),
            ),
          ),
          createElement(
            TableBody,
            null,
            list.map((customer) => createElement(CustomerRow, { key: customer.id, customer })),
          ),
        ),
      ),
      createElement(IconShelf),
    ),
  );
}

// A cache per render, as MUI's server rendering guide has it; with a shared
// one, only the first page would compile its styles.
globalThis.renderDashboard = (rows) => {
  const cache = createCache({ key: "css" });
  const { extractCriticalToChunks, constructStyleTagsFromChunks } = createEmotionServer(cache);
  const html = renderToString(
    createElement(CacheProvider, { value: cache }, createElement(ThemeProvider, { theme }, createElement(Dashboard, { rows }))),
  );
  const styles = constructStyleTagsFromChunks(extractCriticalToChunks(html));
  return `<!DOCTYPE html><html><head>${styles}</head><body><div id="root">${html}</div></body></html>`;
};
